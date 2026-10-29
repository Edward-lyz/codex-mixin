//! Zed's native OS credential slots. Secrets travel through private stdin.
use anyhow::Context;
use serde_json::{Value, json};
use std::process::Command;
use std::time::Duration;

pub fn zed_read(server: &str) -> anyhow::Result<Option<String>> {
    zed_credential(server, "read", None)
}

pub fn zed_write(server: &str, password: Option<&str>) -> anyhow::Result<()> {
    zed_credential(
        server,
        if password.is_some() {
            "write"
        } else {
            "delete"
        },
        password,
    )?;
    Ok(())
}

fn zed_credential(
    server: &str,
    operation: &str,
    password: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let url = reqwest::Url::parse(server)?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
            && !server.contains(['\'', '"', '\\', '\n', '\r']),
        "invalid Zed credential URL"
    );
    anyhow::ensure!(
        password.is_none_or(|value| value.len() <= 8192),
        "Zed credential is too large"
    );
    if cfg!(windows) {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            WINDOWS_CREDENTIAL,
        ]);
        let input = serde_json::to_vec(
            &json!({"operation":operation,"target":format!("zed:url={server}"),"password":password}),
        )?;
        let output = super::agent_cli::run_command_with_input(
            command,
            Duration::from_secs(20),
            "Zed Windows credential",
            Some(input),
        )?;
        let value: Value = serde_json::from_str(output.stdout.trim())
            .context("read native Windows credential result")?;
        return Ok(value.as_str().map(str::to_owned));
    }
    let mut command;
    let mut input = None;
    if cfg!(target_os = "macos") {
        command = Command::new("/usr/bin/security");
        if let Some(password) = password {
            let hex = password
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            command.arg("-i");
            input = Some(
                format!("add-internet-password -U -s '{server}' -a Bearer -X {hex}\n").into_bytes(),
            );
        } else {
            command.args([
                if operation == "read" {
                    "find-internet-password"
                } else {
                    "delete-internet-password"
                },
                "-s",
                server,
                "-a",
                "Bearer",
            ]);
            if operation == "read" {
                command.arg("-w");
            }
        }
    } else {
        command = Command::new("secret-tool");
        command.arg(match operation {
            "read" => "lookup",
            "delete" => "clear",
            _ => "store",
        });
        if operation == "write" {
            command.arg("--label=zed-github-account");
            input = Some(password.context("missing credential")?.as_bytes().to_vec());
        }
        command.args(["url", server, "username", "Bearer"]);
    }
    let result = super::agent_cli::run_command_with_input(
        command,
        Duration::from_secs(20),
        "Zed credential",
        input,
    );
    match result {
        Ok(output) if operation == "read" => Ok(Some(
            output
                .stdout
                .strip_suffix('\n')
                .unwrap_or(&output.stdout)
                .to_owned(),
        )),
        Ok(_) => Ok(None),
        Err(error)
            if operation != "write"
                && (error.to_string().contains("exit status: 44")
                    || error.to_string() == "Zed credential failed with exit status: 1") =>
        {
            Ok(None)
        }
        Err(_) => {
            anyhow::bail!("Zed OS credential operation failed; check the native credential store")
        }
    }
}

// P/Invoke stays in the OS-provided PowerShell bridge; Rust remains safe.
// Neither the JSON input nor the returned password is written to a log.
const WINDOWS_CREDENTIAL: &str = r#"
$ErrorActionPreference='Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ZedCredential {
 [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
 public struct Credential { public uint Flags; public uint Type; public string TargetName; public string Comment; public long LastWritten; public uint CredentialBlobSize; public IntPtr CredentialBlob; public uint Persist; public uint AttributeCount; public IntPtr Attributes; public string TargetAlias; public string UserName; }
 [DllImport("Advapi32.dll",EntryPoint="CredReadW",CharSet=CharSet.Unicode,SetLastError=true)] public static extern bool Read(string target,uint type,uint flags,out IntPtr pointer);
 [DllImport("Advapi32.dll",EntryPoint="CredWriteW",CharSet=CharSet.Unicode,SetLastError=true)] public static extern bool Write(ref Credential credential,uint flags);
 [DllImport("Advapi32.dll",EntryPoint="CredDeleteW",CharSet=CharSet.Unicode,SetLastError=true)] public static extern bool Delete(string target,uint type,uint flags);
 [DllImport("Advapi32.dll")] public static extern void CredFree(IntPtr pointer);
}
'@
$inputValue=[Console]::In.ReadToEnd()|ConvertFrom-Json
$target=[string]$inputValue.target
if($inputValue.operation -eq 'read') {
 $pointer=[IntPtr]::Zero
 if([ZedCredential]::Read($target,1,0,[ref]$pointer)) {
  try { $credential=[Runtime.InteropServices.Marshal]::PtrToStructure($pointer,[type][ZedCredential+Credential]); $value=[Runtime.InteropServices.Marshal]::PtrToStringUni($credential.CredentialBlob,[int]($credential.CredentialBlobSize/2)); ConvertTo-Json -InputObject $value -Compress } finally { [ZedCredential]::CredFree($pointer) }
 } elseif([Runtime.InteropServices.Marshal]::GetLastWin32Error() -eq 1168) { 'null' } else { throw 'credential read failed' }
} elseif($inputValue.operation -eq 'delete') {
 if(-not [ZedCredential]::Delete($target,1,0) -and [Runtime.InteropServices.Marshal]::GetLastWin32Error() -ne 1168) { throw 'credential delete failed' }; 'null'
} else {
 $value=[string]$inputValue.password
 $pointer=[Runtime.InteropServices.Marshal]::StringToCoTaskMemUni($value)
 try { $credential=New-Object ZedCredential+Credential; $credential.Type=1; $credential.TargetName=$target; $credential.CredentialBlobSize=[Text.Encoding]::Unicode.GetByteCount($value); $credential.CredentialBlob=$pointer; $credential.Persist=2; $credential.UserName='Bearer'; if(-not [ZedCredential]::Write([ref]$credential,0)) { throw 'credential write failed' }; 'null' } finally { [Runtime.InteropServices.Marshal]::ZeroFreeCoTaskMemUnicode($pointer) }
}
"#;
