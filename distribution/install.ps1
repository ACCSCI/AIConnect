param(
 [string]$Manifest,
 [string]$PublicKey,
 [string]$Version,
 [string]$InstallDir,
 [string]$EngineUrl,
 [switch]$AllowHttp,
 [switch]$AllowUnsigned,
 [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'
if (-not (Get-Command node -ErrorAction SilentlyContinue)) { throw 'Bootstrap requires Node.js 22.20+. The installed native CLI does not require Node.js.' }
$engine = Join-Path $PSScriptRoot 'install.mjs'
$downloaded = $false
if (-not (Test-Path -LiteralPath $engine)) {
 if (-not $EngineUrl) { $EngineUrl = ([Uri]::new([Uri]$Manifest, '../install.mjs')).AbsoluteUri }
 $engineUri = [Uri]$EngineUrl
 if ($engineUri.Scheme -ne 'https' -and -not ($AllowHttp -and $engineUri.Scheme -eq 'http' -and $engineUri.IsLoopback)) { throw 'Installer engine requires HTTPS' }
 $engine = Join-Path ([IO.Path]::GetTempPath()) ('aiconnect-install-' + [Guid]::NewGuid().ToString('N') + '.mjs')
 $engineDownload = @'
import {writeFile} from 'node:fs/promises';
let url=new URL(process.argv[1]);const path=process.argv[2],local=process.argv[3]==='true';
function check(u){if(u.username||u.password)throw Error('URL credentials forbidden');if(u.protocol==='https:')return;if(local&&u.protocol==='http:'&&['localhost','127.0.0.1','[::1]'].includes(u.hostname))return;throw Error('HTTPS required for installer engine and redirects');}
let downloaded=false;
for(let i=0;i<6;i++){check(url);const r=await fetch(url,{redirect:'manual',signal:AbortSignal.timeout(60000)});if([301,302,303,307,308].includes(r.status)){url=new URL(r.headers.get('location'),url);continue;}if(!r.ok)throw Error('Installer engine HTTP '+r.status);const chunks=[];let size=0;for await(const c of r.body){size+=c.length;if(size>1048576)throw Error('Installer engine too large');chunks.push(c);}await writeFile(path,Buffer.concat(chunks),{flag:'wx',mode:0o600});downloaded=true;break;}if(!downloaded)throw Error('Too many installer engine redirects');
'@
 & node --input-type=module -e $engineDownload $engineUri.AbsoluteUri $engine $AllowHttp.IsPresent.ToString().ToLowerInvariant()
 if ($LASTEXITCODE -ne 0) { throw 'Failed to download installer engine' }
 $downloaded = $true
}
try {
 $engineArgs = @($engine, '--manifest', $Manifest)
 if ($PublicKey) { $engineArgs += @('--public-key', $PublicKey) }
 if ($Version) { $engineArgs += @('--version', $Version) }
 if ($InstallDir) { $engineArgs += @('--install-dir', $InstallDir) }
 if ($AllowHttp) { $engineArgs += '--allow-http' }
 if ($AllowUnsigned) { $engineArgs += '--allow-unsigned' }
 if ($Uninstall) { $engineArgs += '--uninstall' }
 & node @engineArgs
 if ($LASTEXITCODE -ne 0) { throw "AIConnect installer failed ($LASTEXITCODE)" }
} finally { if ($downloaded) { Remove-Item -LiteralPath $engine -Force } }
