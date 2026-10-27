# Desk Host's inbound rule: TCP 8790 from the listed viewer addresses only, private profile.
# Run elevated:
#   powershell -ExecutionPolicy Bypass -File firewall-rule.ps1 -Exe C:\path\to\makepad-app-deskhost.exe -Peers 10.0.0.222
# Remove with: Remove-NetFirewallRule -DisplayName "Makepad Desk Host"
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [string[]]$Peers = @("10.0.0.222"),
    [int]$Port = 8790
)
$ErrorActionPreference = "Stop"
Remove-NetFirewallRule -DisplayName "Makepad Desk Host" -ErrorAction SilentlyContinue
New-NetFirewallRule -DisplayName "Makepad Desk Host" -Direction Inbound -Action Allow -Protocol TCP `
    -LocalPort $Port -RemoteAddress $Peers -Program $Exe -Profile Any | Out-Null
Write-Host "allowed TCP $Port to $Exe from $($Peers -join ', ') (any network profile: the rule names the peers)"
