# Read-only Windows diagnostics. Does not start LM Studio, load models or kill processes.
[CmdletBinding()]
param(
    [string]$OutputPath = (Join-Path ([Environment]::GetFolderPath('Desktop')) 'supa-processes.json')
)

$ErrorActionPreference = 'Stop'
if (-not ('SupaDev.Diagnostics.ProcessAccess' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace SupaDev.Diagnostics {
    public static class ProcessAccess {
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
        [DllImport("kernel32.dll")]
        private static extern bool CloseHandle(IntPtr handle);
        // Opening and immediately closing a handle only checks access; it never terminates.
        public static int TerminateAccessError(uint pid) {
            var handle = OpenProcess(0x0001, false, pid);
            if (handle == IntPtr.Zero) return Marshal.GetLastWin32Error();
            CloseHandle(handle);
            return 0;
        }
    }
}
'@
}

function Get-RuntimeSnapshot {
    $all = @(Get-CimInstance Win32_Process)
    $byId = @{}
    foreach ($process in $all) { $byId[[uint32]$process.ProcessId] = $process }
    $selected = @($all | Where-Object {
        $_.Name -match '^(llmster|lms|llama-server|llmworker|LM Studio|supa-dev[^\\/]*)\.exe$' -or
        $_.ExecutablePath -match '\\(\.lmstudio|LM Studio|lm-studio)\\' -or
        ($_.Name -match '^python(w)?\.exe$' -and
            $_.CommandLine -match '(SupaDev|supa-dev)[\\/].*(manage\.py|server\.py|qwen-work\.py)')
    })
    foreach ($process in $selected) {
        $parent = $byId[[uint32]$process.ParentProcessId]
        # Windows may have reused a dead parent's PID. Never label a newer process as its parent.
        $parentMatches = $null -ne $parent -and $parent.CreationDate -le $process.CreationDate
        $accessError = [SupaDev.Diagnostics.ProcessAccess]::TerminateAccessError($process.ProcessId)
        [pscustomobject]@{
            name = $process.Name
            pid = $process.ProcessId
            created = $process.CreationDate.ToUniversalTime().ToString('o')
            executable = $process.ExecutablePath
            parentPid = $process.ParentProcessId
            parentName = $(if ($parentMatches) { $parent.Name } else { $null })
            parentStillExists = $parentMatches
            workingSetMiB = [math]::Round([double]$process.WorkingSetSize / 1MB, 1)
            privateMiB = [math]::Round([double]$process.PrivatePageCount / 1MB, 1)
            terminateAccessError = $accessError
            terminateAccess = $(if ($accessError -eq 0) { 'allowed (not attempted)' } else { 'unavailable' })
        }
    }
}

$first = @(Get-RuntimeSnapshot)
Start-Sleep -Seconds 3
$second = @(Get-RuntimeSnapshot)
$osInfo = Get-CimInstance Win32_OperatingSystem
$report = [ordered]@{
    collectedAt = [DateTime]::UtcNow.ToString('o')
    windows = $osInfo.Caption
    build = $osInfo.BuildNumber
    elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    first = $first
    second = $second
    note = 'Read-only. No model loaded, no termination attempted. Error 5 means this shell cannot obtain termination access. Access allowed does not prove termination would complete. No command lines, keys, chat history or prompts collected.'
}
$report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $OutputPath -Encoding UTF8
$second | Format-Table name,pid,parentPid,parentName,workingSetMiB,terminateAccessError -AutoSize
Write-Output "Saved read-only report: $OutputPath"
