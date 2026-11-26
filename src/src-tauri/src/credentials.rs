use crate::guardrails::atomic_write_bytes;
use crate::models::RepoCredentialMeta;
use crate::storage;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

fn vault_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCredential {
    id: String,
    provider: String,
    base_url: String,
    label: String,
    username: String,
    token: String,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct VaultPayload {
    version: u32,
    items: Vec<StoredCredential>,
}

const VAULT_MAGIC: &[u8; 8] = b"SFVLT001";

/// Encrypt bytes with Windows DPAPI bound to current user logon session.
#[cfg(windows)]
fn dpapi_protect(plain: &[u8]) -> Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: plain.len() as u32,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 || output.pbData.is_null() {
        return Err(anyhow!("系统 DPAPI 加密凭据失败"));
    }
    let encrypted =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut core::ffi::c_void);
    }
    Ok(encrypted)
}

/// Decrypt bytes previously protected by `dpapi_protect` for the current Windows user.
#[cfg(windows)]
fn dpapi_unprotect(cipher: &[u8]) -> Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: cipher.len() as u32,
        pbData: cipher.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 || output.pbData.is_null() {
        return Err(anyhow!("系统 DPAPI 解密凭据失败（凭据已损坏或不属于当前 Windows 账户）"));
    }
    let plain =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut core::ffi::c_void);
    }
    Ok(plain)
}

#[cfg(not(windows))]
fn dpapi_protect(plain: &[u8]) -> Result<Vec<u8>> {
    Ok(plain.to_vec())
}

#[cfg(not(windows))]
fn dpapi_unprotect(cipher: &[u8]) -> Result<Vec<u8>> {
    Ok(cipher.to_vec())
}

pub fn mask_token(token: &str) -> String {
    let t = token.trim();
    let chars: Vec<char> = t.chars().collect();
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let head: String = chars.iter().take(4).collect();
    let tail: String = chars.iter().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{}****{}", head, tail)
}

/// Scrub any secret token occurrences from error or log strings so tokens never leak.
pub fn scrub_secret(text: &str, token: &str) -> String {
    let t = token.trim();
    if t.len() < 4 {
        return text.to_string();
    }
    text.replace(t, "***")
}

fn load_vault_at(path: &Path) -> Result<VaultPayload> {
    if !path.exists() {
        return Ok(VaultPayload {
            version: 1,
            items: Vec::new(),
        });
    }
    let raw = fs::read(path)?;
    if raw.len() <= VAULT_MAGIC.len() || &raw[..VAULT_MAGIC.len()] != VAULT_MAGIC {
        return Err(anyhow!("凭据库格式无效或已损坏"));
    }
    let plain = dpapi_unprotect(&raw[VAULT_MAGIC.len()..])?;
    let payload: VaultPayload = serde_json::from_slice(&plain)?;
    Ok(payload)
}

fn save_vault_at(path: &Path, payload: &VaultPayload) -> Result<()> {
    let json = serde_json::to_vec(payload)?;
    let cipher = dpapi_protect(&json)?;
    let mut out = Vec::with_capacity(VAULT_MAGIC.len() + cipher.len());
    out.extend_from_slice(VAULT_MAGIC);
    out.extend_from_slice(&cipher);
    atomic_write_bytes(path, &out)
}

fn to_meta(c: &StoredCredential) -> RepoCredentialMeta {
    RepoCredentialMeta {
        id: c.id.clone(),
        provider: c.provider.clone(),
        base_url: c.base_url.clone(),
        label: c.label.clone(),
        username: c.username.clone(),
        masked_token: mask_token(&c.token),
        created_at: c.created_at.clone(),
    }
}

pub fn normalize_provider(p: &str) -> Result<String> {
    match p.trim().to_ascii_lowercase().as_str() {
        "github" => Ok("github".into()),
        "gitee" => Ok("gitee".into()),
        "gitlab" => Ok("gitlab".into()),
        "gitea" | "forgejo" => Ok("gitea".into()),
        other => Err(anyhow!("不支持的仓库平台：{}", other)),
    }
}

pub fn default_base_url(provider: &str) -> &'static str {
    match provider {
        "github" => "https://api.github.com",
        "gitee" => "https://gitee.com",
        "gitlab" => "https://gitlab.com",
        "gitea" => "https://gitea.com",
        _ => "https://api.github.com",
    }
}

pub fn list_credentials() -> Result<Vec<RepoCredentialMeta>> {
    let path = storage::vault_file_path();
    let vault = load_vault_at(&path).unwrap_or_default();
    Ok(vault.items.iter().map(to_meta).collect())
}

pub fn upsert_credential(
    id: Option<String>,
    provider: &str,
    base_url: &str,
    label: &str,
    username: &str,
    token: &str,
) -> Result<RepoCredentialMeta> {
    let prov = normalize_provider(provider)?;
    let tok = token.trim();
    if tok.len() < 8 {
        return Err(anyhow!("访问令牌长度过短，请粘贴完整的个人访问令牌 (PAT)"));
    }
    if tok.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(anyhow!("访问令牌包含非法换行、空格或控制字符，请重新复制"));
    }
    let clean_base = base_url.trim().trim_end_matches('/').to_string();
    let _guard = vault_lock().lock().map_err(|_| anyhow!("凭据库锁异常"))?;
    let path = storage::vault_file_path();
    let mut vault = load_vault_at(&path)?;

    let explicit_id = id.filter(|s| !s.trim().is_empty());
    let matched_idx = vault.items.iter().position(|i| {
        explicit_id.as_deref() == Some(i.id.as_str())
            || (i.provider == prov
                && i.base_url.eq_ignore_ascii_case(&clean_base)
                && !username.trim().is_empty()
                && i.username.eq_ignore_ascii_case(username.trim()))
    });

    // Preserve existing credential ID on username/provider update so project repo_binding.credential_id stays valid
    let cred_id = explicit_id
        .or_else(|| matched_idx.map(|idx| vault.items[idx].id.clone()))
        .unwrap_or_else(|| format!("cred-{}-{}", prov, chrono::Local::now().timestamp_millis()));

    let entry = StoredCredential {
        id: cred_id,
        provider: prov.clone(),
        base_url: clean_base,
        label: if label.trim().is_empty() {
            format!("{} ({})", prov, if username.trim().is_empty() { "Token" } else { username.trim() })
        } else {
            label.trim().to_string()
        },
        username: username.trim().to_string(),
        token: tok.to_string(),
        created_at: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
    };

    if let Some(idx) = matched_idx {
        vault.items[idx] = entry.clone();
    } else {
        vault.items.push(entry.clone());
    }
    save_vault_at(&path, &vault)?;
    Ok(to_meta(&entry))
}

pub fn delete_credential(cred_id: &str) -> Result<Vec<RepoCredentialMeta>> {
    let _guard = vault_lock().lock().map_err(|_| anyhow!("凭据库锁异常"))?;
    let path = storage::vault_file_path();
    let mut vault = load_vault_at(&path)?;
    vault.items.retain(|i| i.id != cred_id);
    save_vault_at(&path, &vault)?;
    Ok(vault.items.iter().map(to_meta).collect())
}

/// Resolve raw token from vault by credential_id, or fallback to matching provider/base_url,
/// or fallback to `gh auth token` for GitHub if installed and logged in.
pub fn resolve_token(provider: &str, base_url: &str, credential_id: &str) -> Result<(String, String)> {
    let prov = normalize_provider(provider)?;
    let clean_base = base_url.trim().trim_end_matches('/');
    let path = storage::vault_file_path();
    if let Ok(vault) = load_vault_at(&path) {
        if !credential_id.trim().is_empty() {
            if let Some(c) = vault.items.iter().find(|i| i.id == credential_id) {
                return Ok((c.token.clone(), c.username.clone()));
            }
        }
        if let Some(c) = vault.items.iter().find(|i| {
            i.provider == prov
                && (clean_base.is_empty()
                    || i.base_url.is_empty()
                    || i.base_url.eq_ignore_ascii_case(clean_base))
        }) {
            return Ok((c.token.clone(), c.username.clone()));
        }
    }

    // Fallback: reuse existing GitHub CLI (`gh auth token`) seamlessly if available
    if prov == "github" && clean_base.is_empty() {
        if let Some(gh_exe) = crate::toolchain::find_binary_native(&["gh.exe"], &[]) {
            let mut cmd = std::process::Command::new(gh_exe);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000);
            }
            if let Ok(out) = cmd.args(["auth", "token"]).output() {
                if out.status.success() {
                    let tok = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !tok.is_empty() {
                        return Ok((tok, "gh-cli".into()));
                    }
                }
            }
        }
    }

    Err(anyhow!(
        "尚未配置 {} 访问令牌。请在「产物与仓库 → 仓库账号」添加令牌后重试。",
        prov
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dpapi_vault_roundtrip_and_no_plaintext_leak() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_vault_test_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(11)
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vault.bin");
        let secret = "ghp_SuperSecretTokenValue987654321";
        let payload = VaultPayload {
            version: 1,
            items: vec![StoredCredential {
                id: "c1".into(),
                provider: "github".into(),
                base_url: String::new(),
                label: "Test".into(),
                username: "alice".into(),
                token: secret.into(),
                created_at: "2026-11-24".into(),
            }],
        };
        save_vault_at(&path, &payload).unwrap();
        let disk_bytes = fs::read(&path).unwrap();
        let disk_str = String::from_utf8_lossy(&disk_bytes);
        #[cfg(windows)]
        assert!(!disk_str.contains(secret), "Token must NEVER appear in plaintext on disk");

        let loaded = load_vault_at(&path).unwrap();
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].token, secret);
        assert_eq!(mask_token(secret), "ghp_****4321");
        assert!(!scrub_secret(&format!("err: {}", secret), secret).contains(secret));

        // Corrupted ciphertext must fail cleanly
        let mut bad = disk_bytes.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        fs::write(&path, &bad).unwrap();
        #[cfg(windows)]
        assert!(load_vault_at(&path).is_err());

        let _ = fs::remove_dir_all(&dir);
    }
}
