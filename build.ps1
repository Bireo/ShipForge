# ShipForge 构建与打包脚本
[CmdletBinding()]
param(
  [switch]$AppOnly,
  [switch]$Clean,
  [switch]$TestsOnly,
  [switch]$SkipTests,
  [ValidateSet('patch', 'minor', 'major')]
  [string]$Bump,
  [string]$Version,
  [ValidateSet('stable', 'beta')]
  [string]$Channel = 'stable'
)

$ErrorActionPreference = 'Stop'
$root      = $PSScriptRoot
$tauriDir  = Join-Path $root 'src\src-tauri'
$confPath  = Join-Path $tauriDir 'tauri.conf.json'
$cargoPath = Join-Path $tauriDir 'Cargo.toml'
$target    = 'D:\Builds\shipforge-target'
$testTgt   = 'D:\Builds\shipforge-test-target'

$env:RUSTUP_TOOLCHAIN = 'gnu-d'
$env:CARGO_HOME       = 'D:\Tool\cargo-ascii'
$env:Path             = 'D:\Tool\binutils\mingw64\bin;' + $env:Path

$buildSw = [Diagnostics.Stopwatch]::StartNew()

function Format-CompactDuration([double]$TotalSeconds) {
  if ([double]::IsNaN($TotalSeconds) -or [double]::IsInfinity($TotalSeconds) -or $TotalSeconds -lt 0) {
    return '0.0s'
  }
  $tenths = [long][Math]::Round($TotalSeconds * 10.0, [MidpointRounding]::AwayFromZero)
  if ($tenths -lt 600) { return ('{0:0.0}s' -f ($tenths / 10.0)) }
  $sec = [long][Math]::Floor($tenths / 10.0)
  $days = [long][Math]::Floor($sec / 86400)
  $rem = $sec % 86400
  $hours = [long][Math]::Floor($rem / 3600)
  $rem = $rem % 3600
  $mins = [long][Math]::Floor($rem / 60)
  $s = $rem % 60
  if ($days -gt 0) { return "${days}d${hours}h${mins}m${s}s" }
  if ($hours -gt 0) { return "${hours}h${mins}m${s}s" }
  return "${mins}m${s}s"
}

function Enable-FastRustLinker {
  try {
    $sysroot = (& rustc --print sysroot 2>$null | Select-Object -First 1)
    if ($sysroot) {
      $lldDir = Join-Path $sysroot.Trim() 'lib\rustlib\x86_64-pc-windows-gnu\bin\gcc-ld'
      $lldExe = Join-Path $lldDir 'ld.lld.exe'
      if (Test-Path -LiteralPath $lldExe) {
        if ($env:Path -notlike "*$lldDir*") { $env:Path = "$lldDir;$env:Path" }
        $curFlags = [string]$env:RUSTFLAGS
        if ($curFlags -notmatch '-fuse-ld=') {
          $env:RUSTFLAGS = (($curFlags.Trim() + ' -C link-arg=-fuse-ld=lld').Trim())
        }
        return $true
      }
    }
  } catch {}
  return $false
}

function Get-Sha256Fast([string]$Path) {
  $sha = [System.Security.Cryptography.SHA256]::Create()
  $fs = New-Object System.IO.FileStream($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read, 262144)
  try {
    return ([BitConverter]::ToString($sha.ComputeHash($fs)).Replace('-', '').ToLowerInvariant())
  } finally {
    $fs.Dispose()
    $sha.Dispose()
  }
}

function Get-Sha256Parallel([string[]]$Paths) {
  $res = @{}
  if (-not $Paths -or $Paths.Count -eq 0) { return $res }
  if ($Paths.Count -eq 1) {
    $res[$Paths[0]] = Get-Sha256Fast $Paths[0]
    return $res
  }
  if (-not ('ShipForgeFastSha' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Concurrent;
using System.IO;
using System.Security.Cryptography;
using System.Threading.Tasks;

public static class ShipForgeFastSha {
  public static ConcurrentDictionary<string, string> ComputeAll(string[] paths) {
    var dict = new ConcurrentDictionary<string, string>(StringComparer.OrdinalIgnoreCase);
    Parallel.ForEach(paths, p => {
      using (var sha = SHA256.Create())
      using (var fs = new FileStream(p, FileMode.Open, FileAccess.Read, FileShare.Read, 262144)) {
        var hash = sha.ComputeHash(fs);
        dict[p] = BitConverter.ToString(hash).Replace("-", "").ToLowerInvariant();
      }
    });
    return dict;
  }
}
'@
  }
  $dict = [ShipForgeFastSha]::ComputeAll($Paths)
  foreach ($kv in $dict.GetEnumerator()) { $res[$kv.Key] = $kv.Value }
  return $res
}

function Get-ShipForgeRustFingerprint {
  $sb = New-Object System.Text.StringBuilder
  $rsFiles = Get-ChildItem -LiteralPath $tauriDir -Recurse -File -Filter '*.rs' | Sort-Object FullName
  foreach ($f in $rsFiles) {
    [void]$sb.AppendLine(($f.FullName.Substring($tauriDir.Length) + '|' + (Get-Sha256Fast $f.FullName)))
  }
  if (Test-Path -LiteralPath $cargoPath) {
    $raw = [IO.File]::ReadAllText($cargoPath, [Text.Encoding]::UTF8)
    $norm = [regex]::Replace($raw, '(?m)^(version\s*=\s*")[^"]+(")', '${1}0.0.0${2}', 1)
    [void]$sb.AppendLine('Cargo.toml|' + $norm)
  }
  $lockPath = Join-Path $tauriDir 'Cargo.lock'
  if (Test-Path -LiteralPath $lockPath) {
    $raw = [IO.File]::ReadAllText($lockPath, [Text.Encoding]::UTF8)
    $norm = [regex]::Replace($raw, '(?ms)(\[\[package\]\]\r?\nname\s*=\s*"shipforge"\r?\nversion\s*=\s*")[^"]+(")(?=(?:\r?\n(?!source\s*=)|\z))', '${1}0.0.0${2}', 1)
    [void]$sb.AppendLine('Cargo.lock|' + $norm)
  }
  if (Test-Path -LiteralPath $confPath) {
    $raw = [IO.File]::ReadAllText($confPath, [Text.Encoding]::UTF8)
    $norm = [regex]::Replace($raw, '("version"\s*:\s*")[^"]+(")', '${1}0.0.0${2}', 1)
    [void]$sb.AppendLine('tauri.conf.json|' + $norm)
  }
  $cargoCfg = Join-Path $tauriDir '.cargo\config.toml'
  if (Test-Path -LiteralPath $cargoCfg) {
    [void]$sb.AppendLine('.cargo/config.toml|' + (Get-Sha256Fast $cargoCfg))
  }
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $bytes = [Text.Encoding]::UTF8.GetBytes($sb.ToString())
    return ([BitConverter]::ToString($sha.ComputeHash($bytes)).Replace('-', '').ToLowerInvariant())
  } finally {
    $sha.Dispose()
  }
}

function Get-Ver {
  return [string](Get-Content $confPath -Raw -Encoding UTF8 | ConvertFrom-Json).version
}

function Set-Ver([string]$v) {
  if ($v -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$') { throw "版本号格式错误：$v" }
  $enc = New-Object Text.UTF8Encoding($false)
  $conf = [IO.File]::ReadAllText($confPath, [Text.Encoding]::UTF8)
  $conf = [regex]::Replace($conf, '("version"\s*:\s*")[^"]+(")', { param($m) $m.Groups[1].Value + $v + $m.Groups[2].Value }, 1)
  [IO.File]::WriteAllText($confPath, $conf, $enc)

  $cargo = [IO.File]::ReadAllText($cargoPath, [Text.Encoding]::UTF8)
  $cargo = [regex]::Replace($cargo, '(?m)^(version\s*=\s*")[^"]+(")', { param($m) $m.Groups[1].Value + $v + $m.Groups[2].Value }, 1)
  [IO.File]::WriteAllText($cargoPath, $cargo, $enc)

  $lockPath = Join-Path $tauriDir 'Cargo.lock'
  if (Test-Path -LiteralPath $lockPath) {
    $lockRaw = [IO.File]::ReadAllText($lockPath, [Text.Encoding]::UTF8)
    $lockNew = [regex]::Replace(
      $lockRaw,
      '(?ms)(\[\[package\]\]\r?\nname\s*=\s*"shipforge"\r?\nversion\s*=\s*")[^"]+(")(?=(?:\r?\n(?!source\s*=)|\z))',
      { param($m) $m.Groups[1].Value + $v + $m.Groups[2].Value },
      1
    )
    if ($lockNew -ne $lockRaw) {
      [IO.File]::WriteAllText($lockPath, $lockNew, $enc)
    }
  }
}

function Next-Ver([string]$cur, [string]$kind) {
  $m = [regex]::Match($cur.Trim(), '^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$')
  if (-not $m.Success) { throw "无法解析当前版本号：$cur" }
  $a = [int]$m.Groups[1].Value; $b = [int]$m.Groups[2].Value; $c = [int]$m.Groups[3].Value
  $pre = $m.Groups[4].Value
  switch ($kind) {
    'major' { return "$($a + 1).0.0" }
    'minor' { return "$a.$($b + 1).0" }
    'patch' {
      if ($pre) { return "$a.$b.$c" }
      return "$a.$b.$($c + 1)"
    }
  }
}

function Invoke-Tests {
  $gateSw = [Diagnostics.Stopwatch]::StartNew()
  Write-Host '[1/3] 运行 ShipForge 测试门禁（全量前端语法 + 单元测试 + Rust 测试并行）…' -ForegroundColor Cyan
  [void](Enable-FastRustLinker)
  $node = (Get-Command node -ErrorAction SilentlyContinue).Source
  $nodeProc = $null; $nodeOut = $null; $nodeErr = $null
  $jsCheckProcs = New-Object System.Collections.Generic.List[object]
  if ($node) {
    # Parallel node --check across all frontend JS files + api.selftest.mjs
    $jsRoot = Join-Path $root 'src\web\js'
    $jsFiles = Get-ChildItem -LiteralPath $jsRoot -Recurse -File -Include '*.js', '*.mjs' | Sort-Object FullName
    $checkRunner = @"
const { execFileSync } = require('child_process');
const files = JSON.parse(process.argv[1]);
for (const f of files) {
  execFileSync(process.execPath, ['--check', f], { stdio: ['ignore', 'pipe', 'pipe'] });
}
console.log('js-syntax: ' + files.length + ' files ok');
"@
    $jsJson = ConvertTo-Json -Compress -InputObject @($jsFiles | ForEach-Object { $_.FullName })
    $psiChk = New-Object System.Diagnostics.ProcessStartInfo
    $psiChk.FileName = $node
    $psiChk.WorkingDirectory = $root
    $psiChk.UseShellExecute = $false
    $psiChk.CreateNoWindow = $true
    $psiChk.RedirectStandardOutput = $true
    $psiChk.RedirectStandardError = $true
    $psiChk.StandardOutputEncoding = [Text.Encoding]::UTF8
    $psiChk.StandardErrorEncoding = [Text.Encoding]::UTF8
    [void]$psiChk.ArgumentList.Add('-e')
    [void]$psiChk.ArgumentList.Add($checkRunner)
    [void]$psiChk.ArgumentList.Add($jsJson)
    $chkProc = New-Object System.Diagnostics.Process
    $chkProc.StartInfo = $psiChk
    [void]$chkProc.Start()
    $jsCheckProcs.Add([pscustomobject]@{
      Name = 'JS语法检查'
      Proc = $chkProc
      Out  = $chkProc.StandardOutput.ReadToEndAsync()
      Err  = $chkProc.StandardError.ReadToEndAsync()
    })

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $node
    $psi.WorkingDirectory = $root
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [Text.Encoding]::UTF8
    $psi.StandardErrorEncoding = [Text.Encoding]::UTF8
    [void]$psi.ArgumentList.Add((Join-Path $root 'src\web\js\api.selftest.mjs'))
    $nodeProc = New-Object System.Diagnostics.Process
    $nodeProc.StartInfo = $psi
    [void]$nodeProc.Start()
    $nodeOut = $nodeProc.StandardOutput.ReadToEndAsync()
    $nodeErr = $nodeProc.StandardError.ReadToEndAsync()

    $webTest = Join-Path $root 'src\web\js\website.selftest.mjs'
    if (Test-Path -LiteralPath $webTest) {
      $psiWeb = New-Object System.Diagnostics.ProcessStartInfo
      $psiWeb.FileName = $node
      $psiWeb.WorkingDirectory = $root
      $psiWeb.UseShellExecute = $false
      $psiWeb.CreateNoWindow = $true
      $psiWeb.RedirectStandardOutput = $true
      $psiWeb.RedirectStandardError = $true
      $psiWeb.StandardOutputEncoding = [Text.Encoding]::UTF8
      $psiWeb.StandardErrorEncoding = [Text.Encoding]::UTF8
      [void]$psiWeb.ArgumentList.Add($webTest)
      $webProc = New-Object System.Diagnostics.Process
      $webProc.StartInfo = $psiWeb
      [void]$webProc.Start()
      $jsCheckProcs.Add([pscustomobject]@{
        Name = '官网与仓库契约自测'
        Proc = $webProc
        Out  = $webProc.StandardOutput.ReadToEndAsync()
        Err  = $webProc.StandardError.ReadToEndAsync()
      })
    }
  }

  $rustStampFile = 'D:\Builds\.shipforge-rust-test-stamp'
  $rustFp = Get-ShipForgeRustFingerprint
  $prevStamp = if (Test-Path -LiteralPath $rustStampFile) { ([IO.File]::ReadAllText($rustStampFile, [Text.Encoding]::UTF8)).Trim() } else { '' }
  $needRust = $TestsOnly -or $Clean -or ($rustFp -ne $prevStamp)

  $rustCode = 0
  $rustWarns = 0
  try {
    if ($needRust) {
      $env:CARGO_TARGET_DIR = $testTgt
      $testOut = & cargo test --manifest-path $cargoPath 2>&1
      $rustCode = $LASTEXITCODE
      $testOut | ForEach-Object { Write-Host $_ }
      $rustWarns += ($testOut | Select-String -Pattern '^warning: ' | Where-Object { $_.Line -notmatch 'generated \d+ warning' } | Measure-Object).Count
      if ($rustCode -eq 0) {
        # Keep cargo check --release strictly in $testTgt (without --target) so it NEVER thrashes $target's
        # cargo-tauri release cache (which builds with tauri/custom-protocol + target triple).
        $env:CARGO_TARGET_DIR = $testTgt
        $chkOut = & cargo check --release --manifest-path $cargoPath 2>&1
        $rustCode = $LASTEXITCODE
        $chkOut | ForEach-Object { Write-Host $_ }
        $rustWarns += ($chkOut | Select-String -Pattern '^warning: ' | Where-Object { $_.Line -notmatch 'generated \d+ warning' } | Measure-Object).Count
      }
      if ($rustCode -eq 0 -and $rustWarns -eq 0) {
        $stampDir = Split-Path -Parent $rustStampFile
        if (-not (Test-Path -LiteralPath $stampDir)) { New-Item -ItemType Directory -Force -Path $stampDir | Out-Null }
        $encUtf8 = New-Object Text.UTF8Encoding($false)
        [IO.File]::WriteAllText($rustStampFile, $rustFp, $encUtf8)
      }
    } else {
      Write-Host '  Rust 源码与依赖指纹未变化，复用已通过的测试凭证（前端全量门禁照常执行）' -ForegroundColor DarkGray
    }

    $nodeCode = 0
    foreach ($jp in $jsCheckProcs) {
      $jp.Proc.WaitForExit()
      $jTxt = ($jp.Out.GetAwaiter().GetResult() + $jp.Err.GetAwaiter().GetResult()).Trim()
      if ($jTxt) { Write-Host "  $jTxt" -ForegroundColor DarkGray }
      if ($jp.Proc.ExitCode -ne 0) { $nodeCode = $jp.Proc.ExitCode }
    }
    if ($nodeProc) {
      $nodeProc.WaitForExit()
      if ($nodeProc.ExitCode -ne 0) { $nodeCode = $nodeProc.ExitCode }
      $txt = ($nodeOut.GetAwaiter().GetResult() + $nodeErr.GetAwaiter().GetResult()).Trim()
      if ($txt) { Write-Host $txt }
    }

    if ($nodeCode -ne 0) { throw '前端语法或自测失败' }
    if ($rustCode -ne 0) { throw 'Rust 单元测试或发布态静态检查失败' }
    if ($rustWarns -gt 0) { throw "Rust 编译存在 $rustWarns 条警告（门禁要求严格 0 警告）" }
  } finally {
    foreach ($jp in $jsCheckProcs) {
      try { if (-not $jp.Proc.HasExited) { $jp.Proc.Kill($true) } } catch {}
      try { $jp.Proc.Dispose() } catch {}
    }
    if ($nodeProc) {
      try { if (-not $nodeProc.HasExited) { $nodeProc.Kill($true) } } catch {}
      try { $nodeProc.Dispose() } catch {}
    }
  }
  $gateSw.Stop()
  Write-Host ("测试门禁全部通过（耗时 {0}）" -f (Format-CompactDuration $gateSw.Elapsed.TotalSeconds)) -ForegroundColor Green
}

if ($Clean) {
  Remove-Item $target -Recurse -Force -ErrorAction SilentlyContinue
  Write-Host '已清理构建缓存' -ForegroundColor DarkGray
}

if ($TestsOnly) {
  Invoke-Tests
  return
}

$cur = Get-Ver
if ($Bump) {
  $cur = Next-Ver $cur $Bump
  Set-Ver $cur
  Write-Host "版本已递增至 $cur" -ForegroundColor Green
} elseif ($Version) {
  Set-Ver $Version
  $cur = $Version
  Write-Host "版本已设置为 $cur" -ForegroundColor Green
}

if (-not $SkipTests) {
  Invoke-Tests
}

Write-Host "[2/3] 编译 ShipForge v$cur…" -ForegroundColor Cyan
$env:CARGO_TARGET_DIR = $target
if (Enable-FastRustLinker) {
  Write-Host '  链接加速器：已启用 rust-lld + 16 核并行编译' -ForegroundColor DarkGray
}

$tauriCli = 'D:\Tool\ShipForge\tools\tauri\cargo-tauri.exe'
if (-not (Test-Path $tauriCli)) { $tauriCli = 'D:\Tools\TidyAssistant\tools\cargo-tauri.exe' }
Push-Location $tauriDir
try {
  if (Test-Path $tauriCli) {
    if ($AppOnly) {
      & $tauriCli build --no-bundle
    } else {
      & $tauriCli build --bundles nsis
    }
  } else {
    & cargo build --release --target x86_64-pc-windows-gnu
  }
  if ($LASTEXITCODE -ne 0) { throw 'Rust / Tauri 构建失败' }
} finally {
  Pop-Location
}

$runningSf = Get-Process -Name shipforge -ErrorAction SilentlyContinue
if ($runningSf) {
  $runningSf | Stop-Process -Force -ErrorAction SilentlyContinue
  $waitDeadline = [DateTime]::UtcNow.AddSeconds(5)
  while ((Get-Process -Name shipforge -ErrorAction SilentlyContinue) -and ([DateTime]::UtcNow -lt $waitDeadline)) {
    Start-Sleep -Milliseconds 150
  }
  Start-Sleep -Milliseconds 250
}

$builtExe = Join-Path $target 'x86_64-pc-windows-gnu\release\shipforge.exe'
if (-not (Test-Path $builtExe)) { throw "未找到编译产物：$builtExe" }
$rootExe = Join-Path $root 'shipforge.exe'
$copied = $false
for ($attempt = 1; $attempt -le 5; $attempt++) {
  try {
    Copy-Item $builtExe $rootExe -Force
    $copied = $true
    break
  } catch {
    if ($attempt -eq 5) { throw "写入主程序 shipforge.exe 失败（可能仍被占用）：$_" }
    Start-Sleep -Milliseconds (300 * $attempt)
  }
}
if (-not $copied) { throw '写入主程序 shipforge.exe 失败' }

# Copy WebView2Loader.dll alongside exe if available
$wv2 = Join-Path $target 'x86_64-pc-windows-gnu\release\WebView2Loader.dll'
if (-not (Test-Path $wv2)) { $wv2 = 'D:\Tools\TidyAssistant\WebView2Loader.dll' }
if (Test-Path $wv2) { Copy-Item $wv2 (Join-Path $root 'WebView2Loader.dll') -Force }

$exeBytes = (Get-Item $rootExe).Length
if ($exeBytes -lt 512KB) { throw "主程序体积异常过小：$exeBytes B" }
Write-Host ("主程序：shipforge.exe ({0:N2} MB)" -f ($exeBytes / 1MB)) -ForegroundColor Green

if ($AppOnly) { return }

Write-Host '[3/3] 整理安装包与便携压缩包并执行产物自检…' -ForegroundColor Cyan
$relDir = Join-Path $root 'release'
New-Item -ItemType Directory -Force -Path $relDir | Out-Null

$nsisDir = Join-Path $target 'x86_64-pc-windows-gnu\release\bundle\nsis'
$setupSrc = Join-Path $nsisDir "ShipForge_${cur}_x64-setup.exe"
$sums = New-Object System.Collections.Generic.List[string]

$hashTargets = New-Object System.Collections.Generic.List[string]
$setupDst = $null
if (Test-Path $setupSrc) {
  $setupDst = Join-Path $relDir "ShipForge_${cur}_x64-setup.exe"
  Copy-Item $setupSrc $setupDst -Force
  $setupLen = (Get-Item $setupDst).Length
  if ($setupLen -lt 512KB) { throw "安装包体积异常过小：$setupLen B" }
  $hashTargets.Add($setupDst)
  Write-Host ("安装包：release\ShipForge_{0}_x64-setup.exe ({1:N2} MB)" -f $cur, ($setupLen / 1MB)) -ForegroundColor Green
} elseif (Test-Path $tauriCli) {
  throw "NSIS 安装包缺失：$setupSrc"
}

# Portable ZIP
$stage = 'D:\Builds\shipforge-portable'
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item $rootExe $stage -Force
if (Test-Path (Join-Path $root 'WebView2Loader.dll')) {
  Copy-Item (Join-Path $root 'WebView2Loader.dll') $stage -Force
}
Copy-Item (Join-Path $root 'README.md') $stage -Force -ErrorAction SilentlyContinue
Copy-Item (Join-Path $root 'LICENSE') $stage -Force -ErrorAction SilentlyContinue
$zipName = "ShipForge_${cur}_x64-portable.zip"
$zipDst = Join-Path $relDir $zipName
$zipTmp = "$zipDst.tmp"
Remove-Item $zipTmp -Force -ErrorAction SilentlyContinue
Add-Type -AssemblyName System.IO.Compression.FileSystem
try {
  [System.IO.Compression.ZipFile]::CreateFromDirectory(
    $stage,
    $zipTmp,
    [System.IO.Compression.CompressionLevel]::Optimal,
    $false
  )

  # Verify ZIP structural integrity & embedded exe exact size before atomic publish
  $zipArch = [System.IO.Compression.ZipFile]::OpenRead($zipTmp)
  try {
    $exeEntry = $zipArch.Entries | Where-Object { $_.FullName -ieq 'shipforge.exe' } | Select-Object -First 1
    if (-not $exeEntry -or $exeEntry.Length -ne $exeBytes) {
      throw "便携包完整性校验失败：shipforge.exe 条目缺失或字节数不匹配"
    }
  } finally {
    $zipArch.Dispose()
  }
  Move-Item -LiteralPath $zipTmp -Destination $zipDst -Force
} catch {
  Remove-Item -LiteralPath $zipTmp -Force -ErrorAction SilentlyContinue
  throw
}

$hashTargets.Add($zipDst)
$shaMap = Get-Sha256Parallel -Paths ($hashTargets.ToArray())
if ($setupDst) { $sums.Add("$($shaMap[$setupDst])  ShipForge_${cur}_x64-setup.exe") }
$sums.Add("$($shaMap[$zipDst])  $zipName")
Write-Host ("便携包：release\{0} ({1:N2} MB)" -f $zipName, ((Get-Item $zipDst).Length / 1MB)) -ForegroundColor Green

$enc = New-Object Text.UTF8Encoding($false)
$sumsPath = Join-Path $relDir 'SHA256SUMS.txt'
$sumsTmp = "$sumsPath.tmp"
[IO.File]::WriteAllText($sumsTmp, ($sums -join "`n") + "`n", $enc)
Move-Item -LiteralPath $sumsTmp -Destination $sumsPath -Force

# Re-verify every entry in SHA256SUMS.txt concurrently against disk
$verifyList = New-Object System.Collections.Generic.List[object]
$verifyPaths = New-Object System.Collections.Generic.List[string]
foreach ($line in (Get-Content -LiteralPath $sumsPath -Encoding UTF8)) {
  if ($line -match '^([0-9a-fA-F]{64})\s+\*?(.+)$') {
    $fName = $Matches[2].Trim()
    $fPath = Join-Path $relDir $fName
    if (-not (Test-Path -LiteralPath $fPath)) { throw "校验表引用文件不存在：$fName" }
    $verifyList.Add([pscustomobject]@{ Name = $fName; Path = $fPath; Expected = $Matches[1].ToLowerInvariant() })
    $verifyPaths.Add($fPath)
  }
}
if ($verifyPaths.Count -gt 0) {
  $recheckMap = Get-Sha256Parallel -Paths ($verifyPaths.ToArray())
  foreach ($v in $verifyList) {
    if ($recheckMap[$v.Path] -ne $v.Expected) { throw "SHA-256 复检不匹配：$($v.Name)" }
  }
}

$pubScript = Join-Path $root 'tools\publish-release.ps1'
if (Test-Path -LiteralPath $pubScript) {
  & $pubScript -Version $cur -Channel $Channel
}

$buildSw.Stop()
Write-Host ("ShipForge v{0} 全量构建与校验完成（总耗时 {1}）" -f $cur, (Format-CompactDuration $buildSw.Elapsed.TotalSeconds)) -ForegroundColor Green
