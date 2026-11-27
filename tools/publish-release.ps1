<#
  ShipForge · 独立发包与在线更新推送脚本
  ------------------------------------------------------------------
  用法：
    1) 生成 GitHub Releases 清单（默认指向 Bireo/ShipForge）：
       pwsh -File .\tools\publish-release.ps1

    2) 生成清单并自动上传 GitHub Release（二进制优先，latest.json 最后上传）：
       pwsh -File .\tools\publish-release.ps1 -UploadGitHub
       pwsh -File .\tools\publish-release.ps1 -Channel beta -UploadGitHub

    3) 生成静态站点 / OSS / 自建源清单：
       pwsh -File .\tools\publish-release.ps1 -BaseUrl "https://cdn.example.com/shipforge/"

    4) 启动本机回环测试更新源（支持 Range 断点续传）：
       pwsh -File .\tools\publish-release.ps1 -ServeLocal -Port 18765

  产物（统一落盘到 release\）：
    - release\latest.json      标准在线更新清单（含安装版 + 便携版 + SHA-256 + 体积 + 更新日志）
    - release\SHA256SUMS.txt   SHA-256 校验摘要表
#>
[CmdletBinding()]
param(
  [string]$Version,
  [ValidateSet('stable', 'beta')]
  [string]$Channel = 'stable',
  [string]$BaseUrl = '',
  [string]$GitHubRepo = 'Bireo/ShipForge',
  [string]$Notes = '',
  [string]$NotesFile = '',
  [string]$MinVersion = '',
  [switch]$UploadGitHub,
  [switch]$Clobber,
  [switch]$ServeLocal,
  [int]$Port = 18765
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$confPath = Join-Path $root 'src\src-tauri\tauri.conf.json'
$releaseDir = Join-Path $root 'release'
$changelogPath = Join-Path $root 'CHANGELOG.md'

if (-not $Version) {
  $conf = Get-Content $confPath -Raw -Encoding UTF8 | ConvertFrom-Json
  $Version = [string]$conf.version
}
if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
  throw "版本号格式无效：$Version"
}

if (-not (Test-Path -LiteralPath $releaseDir)) {
  throw "找不到 release 目录：$releaseDir（请先运行 .\build.ps1 构建产物）"
}

$instFile = "ShipForge_${Version}_x64-setup.exe"
$portFile = "ShipForge_${Version}_x64-portable.zip"
$instPath = Join-Path $releaseDir $instFile
$portPath = Join-Path $releaseDir $portFile

$hasInst = Test-Path -LiteralPath $instPath
$hasPort = Test-Path -LiteralPath $portPath
if (-not $hasInst -and -not $hasPort) {
  throw "release\ 中找不到 v$Version 的安装包或便携包（期望 $instFile 或 $portFile）"
}

function Get-NotesFromChangelog([string]$ver, [string]$path) {
  if (-not (Test-Path -LiteralPath $path)) { return "ShipForge v$ver 正式发布。" }
  $raw = [IO.File]::ReadAllText($path, [Text.Encoding]::UTF8)
  $escaped = [regex]::Escape($ver)
  $m = [regex]::Match($raw, "(?ms)^##\s+(?:v)?${escaped}(?=\s|$|\(|\[|—).*?(?=^##\s+|\z)")
  if (-not $m.Success) { return "ShipForge v$ver 正式发布。" }
  $txt = $m.Value.Trim()
  if ($txt.Length -gt 2400) { $txt = $txt.Substring(0, 2400).TrimEnd() + "`n…" }
  return $txt
}

if (-not $Notes) {
  $Notes = Get-NotesFromChangelog -ver $Version -path $changelogPath
}
$ReleaseBody = $Notes
if ($NotesFile) {
  if (-not (Test-Path -LiteralPath $NotesFile)) {
    throw "找不到 NotesFile：$NotesFile"
  }
  $ReleaseBody = [IO.File]::ReadAllText($NotesFile, [Text.Encoding]::UTF8).Trim()
}

if ($ServeLocal) {
  $BaseUrl = "http://127.0.0.1:$Port/"
} elseif ($BaseUrl) {
  $BaseUrl = $BaseUrl.Trim()
  if (-not $BaseUrl.EndsWith('/')) { $BaseUrl += '/' }
} elseif ($GitHubRepo) {
  $repo = $GitHubRepo.Trim().Trim('/')
  if ($repo -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') {
    throw "GitHubRepo 格式应为 owner/repo：$GitHubRepo"
  }
  $BaseUrl = "https://github.com/$repo/releases/download/v$Version/"
}

function Format-AssetUrl([string]$base, [string]$fileName) {
  if (-not $base) { return $fileName }
  $encoded = [Uri]::EscapeDataString($fileName)
  return "$base$encoded"
}

function Get-Sha256Parallel([string[]]$Paths) {
  if (-not ([System.Management.Automation.PSTypeName]'ShipForgePubSha').Type) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Concurrent;
using System.IO;
using System.Security.Cryptography;
using System.Threading.Tasks;

public static class ShipForgePubSha {
  public static ConcurrentDictionary<string, string> ComputeAll(string[] paths) {
    var dict = new ConcurrentDictionary<string, string>(StringComparer.OrdinalIgnoreCase);
    Parallel.ForEach(paths, p => {
      using (var sha = SHA256.Create())
      using (var fs = new FileStream(p, FileMode.Open, FileAccess.Read, FileShare.Read, 262144)) {
        var h = sha.ComputeHash(fs);
        dict[p] = BitConverter.ToString(h).Replace("-", "").ToLowerInvariant();
      }
    });
    return dict;
  }
}
'@
  }
  return [ShipForgePubSha]::ComputeAll($Paths)
}

$hashTargets = New-Object System.Collections.Generic.List[string]
if ($hasInst) { $hashTargets.Add($instPath) }
if ($hasPort) { $hashTargets.Add($portPath) }
$shaMap = if ($hashTargets.Count -gt 0) { Get-Sha256Parallel -Paths ($hashTargets.ToArray()) } else { @{} }

$manifest = [ordered]@{
  schemaVersion = 1
  version       = $Version
  channel       = $Channel
  pubDate       = (Get-Date).ToString('yyyy-MM-ddTHH:mm:ssK')
  minVersion    = $MinVersion
  notes         = $Notes
}

$sums = New-Object System.Collections.Generic.List[string]

if ($hasInst) {
  $sha = $shaMap[$instPath]
  $len = (Get-Item -LiteralPath $instPath).Length
  $url = Format-AssetUrl $BaseUrl $instFile
  $manifest['installer'] = [ordered]@{
    file   = $instFile
    url    = $url
    size   = $len
    sha256 = $sha
  }
  $manifest['platforms'] = [ordered]@{
    'windows-x86_64' = [ordered]@{
      file   = $instFile
      url    = $url
      size   = $len
      sha256 = $sha
    }
  }
  $sums.Add("$sha  $instFile")
}

if ($hasPort) {
  $sha = $shaMap[$portPath]
  $len = (Get-Item -LiteralPath $portPath).Length
  $url = Format-AssetUrl $BaseUrl $portFile
  $manifest['portable'] = [ordered]@{
    file   = $portFile
    url    = $url
    size   = $len
    sha256 = $sha
  }
  $sums.Add("$sha  $portFile")
}

$enc = New-Object Text.UTF8Encoding($false)
$latestPath = Join-Path $releaseDir 'latest.json'
$sumsPath = Join-Path $releaseDir 'SHA256SUMS.txt'
$jsonText = $manifest | ConvertTo-Json -Depth 6
$latestTmp = "$latestPath.tmp"
$sumsTmp = "$sumsPath.tmp"
[IO.File]::WriteAllText($latestTmp, $jsonText + "`n", $enc)
Move-Item -LiteralPath $latestTmp -Destination $latestPath -Force
[IO.File]::WriteAllText($sumsTmp, (($sums -join "`n") + "`n"), $enc)
Move-Item -LiteralPath $sumsTmp -Destination $sumsPath -Force

Write-Host "已生成发包清单：$latestPath（v$Version / $Channel）" -ForegroundColor Green
Write-Host "已生成校验摘要：$sumsPath" -ForegroundColor DarkGray

if ($UploadGitHub) {
  if (-not $GitHubRepo) { throw '启用 -UploadGitHub 必须同时提供 -GitHubRepo "owner/repo"' }
  $gh = (Get-Command gh -ErrorAction SilentlyContinue).Source
  if (-not $gh) { throw '未找到 GitHub CLI (gh)，请先安装并执行 gh auth login' }

  # 上传前校验二进制魔数与最小体积，杜绝误传残缺或损坏产物
  function Assert-BinaryMagic([string]$p, [byte[]]$magic, [string]$label) {
    $fi = Get-Item -LiteralPath $p
    if ($fi.Length -lt 512KB) { throw "${label}体积异常过小：$($fi.Length) B ($p)" }
    $fs = [IO.File]::OpenRead($p)
    try {
      $buf = New-Object byte[] $magic.Length
      $n = $fs.Read($buf, 0, $magic.Length)
      if ($n -lt $magic.Length) { throw "${label}文件头读取失败：$p" }
      for ($i = 0; $i -lt $magic.Length; $i++) {
        if ($buf[$i] -ne $magic[$i]) { throw "${label}文件头魔数校验失败：$p" }
      }
    } finally {
      $fs.Dispose()
    }
  }
  if ($hasInst) { Assert-BinaryMagic $instPath ([byte[]]@(0x4D, 0x5A)) '安装包' }
  if ($hasPort) { Assert-BinaryMagic $portPath ([byte[]]@(0x50, 0x4B, 0x03, 0x04)) '便携包' }

  # 严格保证原子发布：新建 Release 时先以 --draft 创建并上传二进制与校验表，
  # 再单独串行上传 latest.json，最后解除草稿态，彻底消除并行上传期间客户端拉到 404 资产的竞态窗口。
  $binFiles = @()
  if ($hasInst) { $binFiles += $instPath }
  if ($hasPort) { $binFiles += $portPath }
  $binFiles += $sumsPath

  $tag = "v$Version"
  $tmpBody = Join-Path ([IO.Path]::GetTempPath()) "shipforge_release_notes_${Version}.md"
  [IO.File]::WriteAllText($tmpBody, $ReleaseBody + "`n", $enc)

  try {
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
      & $gh release view $tag --repo $GitHubRepo 2>$null | Out-Null
      $exists = ($LASTEXITCODE -eq 0)
    } finally {
      $ErrorActionPreference = $prevEap
    }
    if ($exists) {
      if (-not $Clobber) {
        throw "远端已存在 Release $tag（如需覆盖资产请加 -Clobber 参数）"
      }
      Write-Host "正在更新已存在的 Release $tag ($GitHubRepo)…" -ForegroundColor Cyan
      & $gh release edit $tag --repo $GitHubRepo --title "ShipForge $tag" --notes-file $tmpBody
      if ($LASTEXITCODE -ne 0) { throw "gh release edit 失败（退出码 $LASTEXITCODE）" }
      foreach ($f in ($binFiles + @($latestPath))) {
        & $gh release upload $tag $f --repo $GitHubRepo --clobber
        if ($LASTEXITCODE -ne 0) { throw "gh release upload 失败：$f（退出码 $LASTEXITCODE）" }
      }
    } else {
      $createArgs = @('release', 'create', $tag) + $binFiles + @('--repo', $GitHubRepo, '--title', "ShipForge $tag", '--notes-file', $tmpBody, '--draft')
      if ($Channel -eq 'beta') { $createArgs += '--prerelease' }
      Write-Host "正在原子上传 Release $tag 到 GitHub ($GitHubRepo)…" -ForegroundColor Cyan
      & $gh @createArgs
      if ($LASTEXITCODE -ne 0) { throw "gh release create (draft) 失败（退出码 $LASTEXITCODE）" }
      & $gh release upload $tag $latestPath --repo $GitHubRepo --clobber
      if ($LASTEXITCODE -ne 0) { throw "gh release upload (latest.json) 失败（退出码 $LASTEXITCODE）" }
      & $gh release edit $tag --repo $GitHubRepo --draft=false
      if ($LASTEXITCODE -ne 0) { throw "gh release publish (--draft=false) 失败（退出码 $LASTEXITCODE）" }
    }
  } finally {
    if (Test-Path -LiteralPath $tmpBody) {
      Remove-Item -LiteralPath $tmpBody -Force -ErrorAction SilentlyContinue
    }
  }
  Write-Host "GitHub Release 发布成功：https://github.com/$GitHubRepo/releases/tag/$tag" -ForegroundColor Green
}

if ($ServeLocal) {
  $listener = [System.Net.HttpListener]::new()
  $prefix = "http://127.0.0.1:$Port/"
  $listener.Prefixes.Add($prefix)
  $listener.Start()
  Write-Host "`n本地更新测试源已启动：$($prefix)latest.json" -ForegroundColor Cyan
  Write-Host "可在启动 ShipForge 前设置 `$env:SHIPFORGE_UPDATE_FEED='$($prefix)latest.json' 进行端到端验证。按 Ctrl+C 停止。" -ForegroundColor Yellow
  try {
    while ($listener.IsListening) {
      $ctx = $listener.GetContext()
      $reqPath = [Uri]::UnescapeDataString($ctx.Request.Url.AbsolutePath.TrimStart('/'))
      $resp = $ctx.Response
      try {
        if (-not $reqPath -or $reqPath.Contains('..') -or $reqPath.Contains('/') -or $reqPath.Contains('\') -or $reqPath.Contains(':')) {
          $resp.StatusCode = 404
          $resp.Close()
          continue
        }
        $target = Join-Path $releaseDir $reqPath
        if (-not (Test-Path -LiteralPath $target -PathType Leaf)) {
          $resp.StatusCode = 404
          $resp.Close()
          continue
        }
        $ext = [IO.Path]::GetExtension($target).ToLowerInvariant()
        $resp.ContentType = switch ($ext) {
          '.json' { 'application/json; charset=utf-8' }
          '.txt'  { 'text/plain; charset=utf-8' }
          '.zip'  { 'application/zip' }
          default { 'application/octet-stream' }
        }
        $resp.Headers['Accept-Ranges'] = 'bytes'
        $fi = Get-Item -LiteralPath $target
        $len = $fi.Length
        $start = 0L
        $end = $len - 1
        $rangeHdr = $ctx.Request.Headers['Range']
        if ($rangeHdr -and $rangeHdr -match '^bytes=(\d+)-(\d*)$') {
          $start = [long]$Matches[1]
          if ($Matches[2]) { $end = [Math]::Min([long]$Matches[2], $len - 1) }
          if ($start -ge $len -or $end -lt $start) {
            $resp.StatusCode = 416
            $resp.Headers['Content-Range'] = "bytes */$len"
            $resp.Close()
            continue
          }
          $resp.StatusCode = 206
          $resp.Headers['Content-Range'] = "bytes $start-$end/$len"
        } else {
          $resp.StatusCode = 200
        }
        $toSend = $end - $start + 1
        $resp.ContentLength64 = $toSend
        $fs = [IO.File]::OpenRead($target)
        try {
          $fs.Seek($start, [IO.SeekOrigin]::Begin) | Out-Null
          $buf = New-Object byte[] 65536
          $rem = $toSend
          while ($rem -gt 0) {
            $read = $fs.Read($buf, 0, [int][Math]::Min($buf.Length, $rem))
            if ($read -le 0) { break }
            $resp.OutputStream.Write($buf, 0, $read)
            $rem -= $read
          }
        } finally {
          $fs.Dispose()
        }
        $resp.Close()
      } catch {
        try { $resp.Abort() } catch {}
      }
    }
  } finally {
    $listener.Stop()
    $listener.Close()
  }
}
