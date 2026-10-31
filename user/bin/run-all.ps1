<#
.SYNOPSIS
Runs gala-os's test programs on Windows and writes a report for
`cargo xtask compare`.

.DESCRIPTION
Runs every *.exe next to this script (PowerShell 5 or later, no admin
rights needed), captures each program's output and exit status, and writes
them as JSON in the schema of `cargo xtask report` (see
xtask/src/report.rs). Programs report through their exit status: 0, the
number of the failing check, or an NTSTATUS if they crash.

The results are meant for the pinned build (Windows 11 24H2, 26100): the
programs use its raw system call numbers.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File .\run-all.ps1 -Out windows.json
#>
param(
    [string]$Out = (Join-Path (Get-Location) "windows.json"),
    [int]$TimeoutSeconds = 60
)
Set-StrictMode -Version 2
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

# A crashing program (crash.exe crashes on purpose) must not open a Windows
# Error Reporting dialog; the error mode is inherited by child processes.
Add-Type -Namespace Gala -Name Native -MemberDefinition @'
[DllImport("kernel32.dll")] public static extern uint SetErrorMode(uint mode);
'@
# SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX
[void][Gala.Native]::SetErrorMode(0x0001 -bor 0x0002 -bor 0x8000)

# Command lines that scenarios pass (gala: `args=` in xtask/src/main.rs).
# nginx.exe would serve until stopped: -v prints its version and exits.
$arguments = @{ "args_test" = 'alpha "beta gamma"'; "go_http" = 'self'; "nginx" = '-v'; "busybox" = 'sh -c "echo hello | cat; echo pipe=$?; echo abc | sed s/b/X/; echo count=$(printf ''x\ny\nx\n'' | grep -c x); exit 3"' }
# smp_test takes an expected processor count (cpus=N); none on Windows.

$results = @()
foreach ($exe in Get-ChildItem -Path $here -Filter *.exe | Sort-Object Name) {
    $name = [IO.Path]::GetFileNameWithoutExtension($exe.Name)
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exe.FullName
    # import_test and loader_test load testdll*.dll from the executable's
    # directory.
    $psi.WorkingDirectory = $here
    if ($arguments.ContainsKey($name)) { $psi.Arguments = $arguments[$name] }
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.CreateNoWindow = $true
    $process = [System.Diagnostics.Process]::Start($psi)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        $process.Kill()
        $process.WaitForExit()
        $results += [ordered]@{
            program = $name; exit = $null; line = ""; skipped = "timed out after $TimeoutSeconds s"
        }
        Write-Host ("{0,-18} timed out" -f $name)
        continue
    }
    $process.WaitForExit()
    $text = $stdout.Result
    # ExitCode is signed; report the 32-bit value as unsigned (NTSTATUS).
    $code = [BitConverter]::ToUInt32([BitConverter]::GetBytes([int32]$process.ExitCode), 0)
    $lines = @($text -split "`r?`n")
    $line = $lines | Where-Object { $_ -like "${name}: *" } | Select-Object -First 1
    if ($null -eq $line) { $line = $lines | Select-Object -First 1 }
    if ($null -eq $line) { $line = "" }
    $results += [ordered]@{ program = $name; exit = $code; line = $line }
    # The benchmarks' numbers, for `cargo xtask bench <this file>`.
    if ($name -eq "bench") {
        $text | Set-Content -Path (Join-Path (Split-Path -Parent $Out) "bench-windows.txt") -Encoding UTF8
    }
    Write-Host ("{0,-18} {1:x8}  {2}" -f $name, $code, $line)
}

$os = Get-CimInstance Win32_OperatingSystem
$report = [ordered]@{ platform = "windows " + $os.Version; results = $results }
$report | ConvertTo-Json -Depth 4 | Set-Content -Path $Out -Encoding UTF8
Write-Host "wrote $Out"
