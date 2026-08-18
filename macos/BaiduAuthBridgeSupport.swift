import Cocoa

func baiduBridgeDisplayName(_ mode: BaiduAuthBridgeMode) -> String {
    switch mode {
    case .disabled: return AppLocalization.string("providerSettings.authBridge2")
    case .ducxLoopback: return "DUCX"
    }
}

func appendBaiduAuthBridgeArguments(
    _ arguments: inout [String],
    mode: BaiduAuthBridgeMode,
    executable: URL? = nil
) {
    arguments.append(contentsOf: ["--baidu-auth-bridge", mode.rawValue])
    if let executable {
        appendBaiduAuthBridgeExecutable(
            &arguments,
            mode: mode,
            executable: executable
        )
    }
}

func appendBaiduAuthBridgeExecutable(
    _ arguments: inout [String],
    mode: BaiduAuthBridgeMode,
    executable: URL
) {
    switch mode {
    case .ducxLoopback:
        arguments.append(contentsOf: ["--ducx-executable", executable.path])
    case .disabled:
        break
    }
}

func managedDucxExecutableURL(
    homeDirectory: URL = FileManager.default.homeDirectoryForCurrentUser
) -> URL {
    homeDirectory
        .appendingPathComponent(
            ".codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/ducx",
            isDirectory: false
        )
}

/// Run the managed DUCX install + QR login in a dedicated Terminal via the
/// bundled `codex-mixin connect ducx`, then return the managed executable.
func setupDucxInTerminal() async throws -> URL {
    guard let cli = Bundle.main.resourceURL?
        .appendingPathComponent("codex-mixin")
    else {
        throw GatewayError.command("无法定位 codex-mixin 可执行文件。")
    }
    let executable = managedDucxExecutableURL()
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("codex-mixin-ducx-setup-\(UUID().uuidString)")
    let script = directory.appendingPathComponent("Configure DUCX.command")
    let loginStatus = directory.appendingPathComponent("login.status")
    let terminalTitle = "Codex Mixin DUCX \(UUID().uuidString)"
    try FileManager.default.createDirectory(
        at: directory,
        withIntermediateDirectories: true,
        attributes: [.posixPermissions: 0o700]
    )
    var setupCompleted = false
    defer {
        if setupCompleted {
            try? FileManager.default.removeItem(at: directory)
        } else {
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 60) {
                try? FileManager.default.removeItem(at: directory)
            }
        }
    }
    let contents = """
    #!/bin/zsh
    printf '\\033]0;\(terminalTitle)\\007'
    echo 'Codex Mixin — DUCX 隔离下载与扫码登录'
    echo '================================'
    \(shellQuoted(cli.path)) connect ducx
    login_result=$?
    printf '%s' "$login_result" > \(shellQuoted(loginStatus.path))
    if [[ "$login_result" -ne 0 ]]; then
      echo
      echo "DUCX 配置失败（退出码 $login_result）。按任意键关闭。"
      read -k 1
      exit "$login_result"
    fi
    echo 'DUCX 配置完成，正在返回 Codex Mixin 应用...'
    (
      sleep 1
      /usr/bin/osascript \\
        -e 'tell application "Terminal"' \\
        -e 'repeat with candidateWindow in windows' \\
        -e 'if name of candidateWindow contains "\(terminalTitle)" then close candidateWindow' \\
        -e 'end repeat' \\
        -e 'end tell'
    ) >/dev/null 2>&1 &!
    exit 0
    """
    try contents.write(to: script, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes(
        [.posixPermissions: 0o700],
        ofItemAtPath: script.path
    )
    guard NSWorkspace.shared.open(script) else {
        throw GatewayError.command("无法打开 Terminal 配置 DUCX。")
    }
    let loginResult = try await waitForBridgeStatus(
        at: loginStatus,
        stage: "DUCX 登录",
        timeoutSeconds: 1_800
    )
    guard loginResult == 0 else {
        throw GatewayError.command("DUCX 配置未成功完成（退出码 \(loginResult)）。")
    }
    guard FileManager.default.isExecutableFile(atPath: executable.path) else {
        throw GatewayError.command("DUCX 配置完成，但托管入口不可执行。")
    }
    setupCompleted = true
    return executable
}

func waitForBridgeStatus(
    at status: URL,
    stage: String,
    timeoutSeconds: Int
) async throws -> Int32 {
    for _ in 0..<(timeoutSeconds * 4) {
        if let value = try? String(contentsOf: status, encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines),
           let result = Int32(value)
        {
            return result
        }
        try await Task.sleep(nanoseconds: 250_000_000)
    }
    throw GatewayError.command("等待\(stage)超时。")
}

func shellQuoted(_ value: String) -> String {
    "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
}
