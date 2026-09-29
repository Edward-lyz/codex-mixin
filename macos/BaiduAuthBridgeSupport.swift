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

/// Install and sign in to the managed DUCX through the CLI, which shows the
/// QR-code login in a Terminal window when needed, and return its executable.
func setupManagedDucx(run: ([String]) async throws -> String) async throws -> URL {
    let output = try await run(["connect", "ducx", "--json"])
    guard
        let response = try JSONSerialization.jsonObject(with: Data(output.utf8)) as? [String: Any],
        let path = response["executable"] as? String,
        !path.isEmpty
    else {
        throw GatewayError.command("DUCX 配置返回了无效 JSON。")
    }
    return URL(fileURLWithPath: path)
}
