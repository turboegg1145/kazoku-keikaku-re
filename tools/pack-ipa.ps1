param(
    [Parameter(Mandatory = $true)][string]$GameDir,
    [string]$Ipa = "",
    [string]$Out = ""
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo = Split-Path -Parent $root

if (-not $Ipa) {
    foreach ($c in @((Join-Path $repo "Kzn.ipa"), (Join-Path $root "Kzn.ipa"))) {
        if (Test-Path $c) { $Ipa = $c; break }
    }
}
if (-not $Ipa -or -not (Test-Path $Ipa)) {
    throw "找不到 Kzn.ipa。把它放到这个脚本旁边（或仓库根目录）再试。"
}
if (-not $Out) { $Out = Join-Path (Split-Path -Parent $Ipa) "Kzn-完整版.ipa" }

$GameDir = (Resolve-Path $GameDir).Path
Write-Host "游戏目录: $GameDir"
Write-Host "基础 ipa : $Ipa"

# 只要这些（其它是汉化补丁、工具、说明，引擎用不上）
$wanted = @("KIZUNAR.EXE", "GGD", "GGD2", "DATA", "ISF", "SE", "VOICE1", "VOICE2", "WMSC", "KIZUNAR.SAV")
$found = @()
foreach ($n in $wanted) {
    $p = Join-Path $GameDir $n
    if (Test-Path $p) { $found += $n } else { Write-Host "  (没有 $n，跳过)" }
}
if (-not ($found -contains "KIZUNAR.EXE")) {
    throw "$GameDir 里没有 KIZUNAR.EXE，选错文件夹了？"
}

$tmp = Join-Path $env:TEMP ("kznpack_" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host "解包 ipa ..."
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::ExtractToDirectory($Ipa, $tmp)

    $appDir = Join-Path $tmp "Payload\Kzn.app"
    if (-not (Test-Path $appDir)) { throw "ipa 结构不对：没有 Payload/Kzn.app" }
    $dst = Join-Path $appDir "gamedata"
    New-Item -ItemType Directory -Path $dst -Force | Out-Null

    $total = 0
    foreach ($n in $found) {
        $p = Join-Path $GameDir $n
        Write-Host "  打包 $n ..."
        Copy-Item -Path $p -Destination $dst -Recurse -Force
    }
    Get-ChildItem -Path $GameDir -Filter "*.MPG" -File | ForEach-Object {
        Write-Host ("  打包 " + $_.Name + " ...")
        Copy-Item -Path $_.FullName -Destination $dst -Force
    }
    $total = (Get-ChildItem -Path $dst -Recurse -File | Measure-Object -Property Length -Sum).Sum

    if (Test-Path $Out) { Remove-Item $Out -Force }
    $zip = $Out + ".zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Write-Host "压缩（约 $([math]::Round($total / 1MB)) MB，要等一会儿）..."
    [System.IO.Compression.ZipFile]::CreateFromDirectory($tmp, $zip, [System.IO.Compression.CompressionLevel]::Optimal, $false)
    Move-Item -Path $zip -Destination $Out -Force
    $size = (Get-Item $Out).Length
    Write-Host ""
    Write-Host "完成: $Out"
    Write-Host ("大小: " + [math]::Round($size / 1MB, 1) + " MB")
    Write-Host "用 TrollStore 装这个，装完直接能玩，不用拷数据。"
}
finally {
    if (Test-Path $tmp) { Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue }
}
