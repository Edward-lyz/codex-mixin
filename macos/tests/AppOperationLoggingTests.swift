import Foundation

@main
struct AppOperationLoggingTests {
    static func main() {
        let command = diagnosticCommandDescription([
            "providers",
            "add",
            "--preset",
            "custom",
            "--key",
            "super-secret",
            "--display-name",
            "AI Hub",
            "--gateway-key=another-secret",
        ])
        precondition(command.contains("providers add"))
        precondition(command.contains("--key=<redacted>"))
        precondition(command.contains("--gateway-key=<redacted>"))
        precondition(command.contains("\"AI Hub\""))
        precondition(!command.contains("super-secret"))
        precondition(!command.contains("another-secret"))

        let safeError = diagnosticSafeText(
            """
            --api-key command-secret
            {"api_key":"json-secret","access_token":"token-secret"}
            password = "config-secret"
            Authorization: Bearer bearer-secret
            """
        )
        for secret in [
            "command-secret",
            "json-secret",
            "token-secret",
            "config-secret",
            "bearer-secret",
        ] {
            precondition(!safeError.contains(secret), "Diagnostic text leaked \(secret)")
        }

        let installSummary = diagnosticOutputSummary(
            arguments: ["install-codex", "--custom-only"],
            output: """
            models installed: 23
            codex validation: debug models loaded 23 models; expected 23; missing 0
            api_key = "future-secret"
            """
        )
        precondition(installSummary.contains("models installed: 23"))
        precondition(installSummary.contains("missing 0"))
        precondition(!installSummary.contains("future-secret"))

        let quotaSummary = diagnosticOutputSummary(
            arguments: ["quota", "--json"],
            output: #"{"raw":{"private_account_field":"secret"},"used":1}"#
        )
        precondition(quotaSummary.contains("output_bytes="))
        precondition(!quotaSummary.contains("private_account_field"))
        precondition(!quotaSummary.contains("secret"))

        let logDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("codex-mixin-log-test-\(UUID().uuidString)")
        appendAppDiagnosticLog(
            "first step api_key=log-secret",
            directory: logDirectory
        )
        appendAppDiagnosticLog("second step", directory: logDirectory)
        let log = try! String(
            contentsOf: logDirectory.appendingPathComponent("gateway.log"),
            encoding: .utf8
        )
        precondition(log.contains("APP_DIAGNOSTIC first step"))
        precondition(log.contains("APP_DIAGNOSTIC second step"))
        precondition(!log.contains("log-secret"))
        let permissions = try! FileManager.default.attributesOfItem(
            atPath: logDirectory.appendingPathComponent("gateway.log").path
        )[.posixPermissions] as! NSNumber
        precondition(permissions.intValue == 0o600)
        try? FileManager.default.removeItem(at: logDirectory)
        print("App operation logging: passed")
    }
}
