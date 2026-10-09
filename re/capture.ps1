# Capture USB traffic for one sdk_probe.py scenario.
#
# Finds the camera's current USBPcap root hub and device address (both change
# across replugs/reboots), records only that device, runs the scenario, then
# decodes the control transfers next to the pcap.
#
# Requires an elevated shell (USBPcap filter devices are admin-only).
#
# Usage: re/capture.ps1 <name> <scenario> [-SnapLen 65535]

param(
    [Parameter(Mandatory)] [string] $Name,
    [Parameter(Mandatory)] [string] $Scenario,
    [int] $SnapLen = 65535
)

$ErrorActionPreference = 'Stop'
$usbpcap = 'C:\Program Files\USBPcap\USBPcapCMD.exe'
$root = Split-Path $PSScriptRoot
$out = Join-Path $root "captures\$Name"

$iface = $null; $addr = $null
foreach ($i in 1..8) {
    $dev = "\\.\USBPcap$i"
    $line = & $usbpcap --extcap-interface $dev --extcap-config 2>$null |
        Select-String '\{display=\[(\d+)\] (Formlabs Form2|USB3\.0 Camera)\}' | Select-Object -First 1
    if ($line) { $iface = $dev; $addr = $line.Matches[0].Groups[1].Value; break }
}
if (-not $iface) { throw 'Camera not found on any USBPcap interface' }
Write-Host "Camera at $iface address $addr"

if (Get-Process amscope -ErrorAction SilentlyContinue) { throw 'AmScope app is running; close it first' }

# USBPcapCMD silently refuses to overwrite an existing file.
Remove-Item "$out.*" -ErrorAction SilentlyContinue

$job = Start-Job { & $using:usbpcap -d $using:iface --devices $using:addr --inject-descriptors -s $using:SnapLen -o "$using:out.pcap" }
Start-Sleep 2
try {
    python (Join-Path $PSScriptRoot 'sdk_probe.py') $Scenario "$out.jsonl"
} finally {
    Start-Sleep 1
    Stop-Process -Name USBPcapCMD -Force -ErrorAction SilentlyContinue
    Remove-Job $job -Force
}
python (Join-Path $PSScriptRoot 'usbpcap.py') "$out.pcap" --jsonl "$out.ctrl.jsonl" > "$out.ctrl.txt"
Write-Host "Wrote $out.{pcap,jsonl,ctrl.jsonl,ctrl.txt}"
