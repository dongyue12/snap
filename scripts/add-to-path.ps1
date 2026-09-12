<#
  把 snap.exe 所在目录加入 PATH。

  用法（通常在 add-to-path.bat 里调用，也可以直接用 PowerShell 跑）：
      add-to-path.ps1                    加入当前用户的 PATH（不需要管理员）
      add-to-path.ps1 -Scope Machine     加入系统 PATH（需要管理员）
      add-to-path.ps1 -Remove            从 PATH 中移除
      add-to-path.ps1 -DryRun            只显示会做什么，不真正修改
      add-to-path.ps1 -Directory <dir>   指定 snap.exe 所在目录（默认脚本所在目录）

  为什么不用 setx：setx 在 PATH 超过 1024 字符时会截断，而且会把系统与用户
  PATH 合并写回用户 PATH。这里改成直接读写注册表，并保留 %变量% 的展开语义。
#>
[CmdletBinding()]
param(
    [ValidateSet('User', 'Machine')][string]$Scope = 'User',
    [switch]$Remove,
    [switch]$DryRun,
    [string]$Directory
)

$ErrorActionPreference = 'Stop'

if (-not $Directory) { $Directory = Split-Path -Parent $MyInvocation.MyCommand.Path }
$Directory = $Directory.TrimEnd('\')

$exe = Join-Path $Directory 'snap.exe'
if (-not (Test-Path -LiteralPath $exe)) {
    Write-Host "[错误] 这个目录里没有 snap.exe：$Directory" -ForegroundColor Red
    Write-Host "       请把本脚本与 snap.exe 放在同一个目录，或用 -Directory 指定。" -ForegroundColor Red
    exit 1
}

$key = if ($Scope -eq 'User') {
    'HKCU:\Environment'
} else {
    'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
}
$scopeName = if ($Scope -eq 'User') { '当前用户' } else { '所有用户（需要管理员）' }

# 直接从注册表读原值：这样 %USERPROFILE% 这类引用不会被提前展开
$current = ''
$item = Get-ItemProperty -Path $key -Name Path -ErrorAction SilentlyContinue
if ($item -and $item.Path) { $current = [string]$item.Path }

$parts = @($current -split ';' | Where-Object { $_ -ne '' })
$same = { param($a, $b) $a.TrimEnd('\') -ieq $b.TrimEnd('\') }

$found = $false
foreach ($p in $parts) { if (& $same $p $Directory) { $found = $true; break } }

Write-Host "目标目录: $Directory"
Write-Host "作用范围: $scopeName"
Write-Host ""

if ($Remove) {
    if (-not $found) {
        Write-Host "本来就不在 PATH 里，无需移除。"
        exit 0
    }
    $new = (@($parts | Where-Object { -not (Test-SamePath $_ $Directory) }) -join ';')
    $action = '移除'
} else {
    if ($found) {
        Write-Host "已经在 PATH 里了，无需重复添加。"
        & $exe --version
        exit 0
    }
    $new = (@($parts + $Directory) -join ';')
    $action = '添加'
}

if ($DryRun) {
    Write-Host "[演练] 将要$action：$Directory"
    Write-Host "[演练] 修改后的 PATH：$new"
    exit 0
}

# 备份原值，便于回退
$backup = Join-Path $env:TEMP ("path-backup-$Scope-$(Get-Date -Format 'yyyyMMdd-HHmmss').txt")
Set-Content -LiteralPath $backup -Value $current -Encoding UTF8
Write-Host "原 PATH 已备份到: $backup"

try {
    # 写成 ExpandString，保留 %变量% 的展开语义
    if (-not (Test-Path -Path $key)) { New-Item -Path $key -Force | Out-Null }
    Set-ItemProperty -Path $key -Name Path -Value $new -Type ExpandString
} catch {
    Write-Host "[失败] 写入注册表失败：$($_.Exception.Message)" -ForegroundColor Red
    if ($Scope -eq 'Machine') {
        Write-Host "       系统级 PATH 需要以管理员身份运行本脚本。" -ForegroundColor Red
    }
    exit 1
}

# 通知系统环境已变更，让新开的窗口立刻生效（setx 内部也是这么做的）
try {
    Add-Type -Namespace Snap -Name Win32 -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint Msg, System.UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out System.UIntPtr lpdwResult);
'@ -ErrorAction Stop
    $result = [System.UIntPtr]::Zero
    [Snap.Win32]::SendMessageTimeout([System.IntPtr]0xffff, 0x1A, [System.UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result) | Out-Null
} catch {
    # 通知失败不影响 PATH 已经写好的事实，新开窗口或重新登录后即可生效
}

Write-Host ""
Write-Host "[成功] 已$action $(if ($Scope -eq 'User') { '用户' } else { '系统' }) PATH。" -ForegroundColor Green
Write-Host "       新开一个终端窗口即可直接使用 snap 命令。"
Write-Host "       撤销：再运行一次并加 /remove（或 -Remove）"
Write-Host ""
& $exe --version
