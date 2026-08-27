import Cocoa

extension AppDelegate {
    func restartGatewayProcess() async throws {
        if FileManager.default.fileExists(atPath: launchAgentPath().path) {
            try await bootoutIfLoaded(launchDomainAndLabel())
            _ = try await runGateway(["stop"])
            try await waitForGatewayStopped()
            try installLaunchAgent()
            try await bootstrapLaunchAgent()
            return
        }
        _ = try await runGateway(["stop"])
        try await waitForGatewayStopped()
        _ = try await runGateway(["start", "--daemon"])
    }

    func waitForGatewayStatus() async throws -> String {
        var lastError = "网关尚未报告健康状态"
        for _ in 0..<20 {
            do {
                let status = try await runGateway(["status"])
                if status.contains("gateway: running") {
                    return status
                }
                lastError = status
            } catch {
                lastError = String(describing: error)
            }
            try await Task.sleep(nanoseconds: 250_000_000)
        }
        throw GatewayError.command("网关启动后 5 秒内未就绪：\(lastError)")
    }

    func waitForGatewayStopped() async throws {
        let runtimeURL = stateDir().appendingPathComponent("runtime.json")
        for _ in 0..<20 {
            guard FileManager.default.fileExists(atPath: runtimeURL.path) else {
                return
            }
            let data = try Data(contentsOf: runtimeURL)
            guard
                let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                let pid = object["pid"] as? NSNumber
            else {
                throw GatewayError.command("无法读取网关 runtime PID：\(runtimeURL.path)")
            }
            if kill(pid.int32Value, 0) != 0 {
                let errorCode = errno
                if errorCode == ESRCH {
                    return
                }
                if errorCode != EPERM {
                    throw GatewayError.command("检查网关进程 \(pid) 失败：errno \(errorCode)")
                }
            }
            try await Task.sleep(nanoseconds: 250_000_000)
        }
        throw GatewayError.command("网关在 5 秒内未停止，可能存在不受 Codex Mixin 管理的进程。")
    }
    func runGateway(_ arguments: [String]) async throws -> String {
        let cliArguments = ["--no-tui"] + arguments
        let operationID = String(UUID().uuidString.prefix(8))
        let command = diagnosticCommandDescription(cliArguments)
        let startedAt = Date()
        appendDiagnosticLog(
            "APP_OPERATION started id=\(operationID) command=\(command.isEmpty ? "<default>" : command)"
        )
        do {
            let output = try await runProcess(try gatewayExecutableURL().path, cliArguments)
            let durationMilliseconds = Int(Date().timeIntervalSince(startedAt) * 1_000)
            appendDiagnosticLog(
                """
                APP_OPERATION completed id=\(operationID) duration_ms=\(durationMilliseconds) command=\(command.isEmpty ? "<default>" : command)
                \(diagnosticOutputSummary(arguments: arguments, output: output))
                """
            )
            return output
        } catch {
            let durationMilliseconds = Int(Date().timeIntervalSince(startedAt) * 1_000)
            appendDiagnosticLog(
                """
                APP_OPERATION failed id=\(operationID) duration_ms=\(durationMilliseconds) command=\(command.isEmpty ? "<default>" : command)
                \(diagnosticErrorDescription(error))
                """
            )
            throw error
        }
    }

    func runGatewayStreaming(_ arguments: [String], onProgress: @escaping (String) -> Void) async throws -> String {
        let executable = try gatewayExecutableURL().path
        return try await runProcessStreaming(executable, ["--no-tui"] + arguments, onProgress: onProgress)
    }

    func bootoutIfLoaded(_ domainAndLabel: String) async throws {
        do {
            _ = try await runProcess("/bin/launchctl", ["bootout", domainAndLabel])
        } catch {
            let message = String(describing: error)
            if !message.contains("No such process") && !message.contains("Could not find service") {
                throw error
            }
        }
    }

    func bootstrapLaunchAgent() async throws {
        _ = try await retryLaunchAgentBootstrap {
            try await runProcess(
                "/bin/launchctl",
                ["bootstrap", launchDomain(), launchAgentPath().path]
            )
        }
    }

    func runProcess(_ executable: String, _ arguments: [String]) async throws -> String {
        let operationID = String(UUID().uuidString.prefix(8))
        let command = diagnosticCommandDescription(arguments)
        let startedAt = Date()
        appendDiagnosticLog(
            "APP_PROCESS started id=\(operationID) executable=\(executable) arguments=\(command)"
        )
        let diagnosticDirectory = stateDir()
        return try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                let process = Process()
                let outputPipe = Pipe()
                process.executableURL = URL(fileURLWithPath: executable)
                process.arguments = arguments
                process.standardOutput = outputPipe
                process.standardError = outputPipe
                var environment = ProcessInfo.processInfo.environment
                let ignoredKeys = environment.keys.filter { key in
                    key.hasPrefix("CODEX_GATEWAY_")
                    || key == "ANTHROPIC_BASE_URL"
                    || key == "ANTHROPIC_API_KEY"
                }
                for key in ignoredKeys {
                    environment.removeValue(forKey: key)
                }
                process.environment = environment
                do {
                    let result = try runProcessCollectingMergedOutput(
                        process,
                        outputPipe: outputPipe,
                        timeout: 600
                    )
                    let output = String(data: result.data, encoding: .utf8) ?? ""
                    let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
                    let durationMilliseconds = Int(Date().timeIntervalSince(startedAt) * 1_000)
                    if result.terminationStatus == 0 {
                        appendAppDiagnosticLog(
                            """
                            APP_PROCESS completed id=\(operationID) duration_ms=\(durationMilliseconds) exit=0 executable=\(executable) arguments=\(command) output_bytes=\(output.lengthOfBytes(using: .utf8))
                            """,
                            directory: diagnosticDirectory
                        )
                        continuation.resume(returning: trimmed)
                    } else {
                        appendAppDiagnosticLog(
                            """
                            APP_PROCESS failed id=\(operationID) duration_ms=\(durationMilliseconds) exit=\(result.terminationStatus) executable=\(executable) arguments=\(command) output_bytes=\(output.lengthOfBytes(using: .utf8))
                            """,
                            directory: diagnosticDirectory
                        )
                        continuation.resume(throwing: GatewayError.command(trimmed.isEmpty ? "exit \(result.terminationStatus)" : trimmed))
                    }
                } catch {
                    let durationMilliseconds = Int(Date().timeIntervalSince(startedAt) * 1_000)
                    appendAppDiagnosticLog(
                        """
                        APP_PROCESS launch_failed id=\(operationID) duration_ms=\(durationMilliseconds) executable=\(executable) arguments=\(command)
                        \(diagnosticErrorDescription(error))
                        """,
                        directory: diagnosticDirectory
                    )
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    func runProcessStreaming(_ executable: String, _ arguments: [String], onProgress: @escaping (String) -> Void) async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                let process = Process()
                let outputPipe = Pipe()
                let errorPipe = Pipe()
                process.executableURL = URL(fileURLWithPath: executable)
                process.arguments = arguments
                process.standardOutput = outputPipe
                process.standardError = errorPipe
                let collector = StreamingProcessOutputCollector(
                    progressPrefix: "MIXIN_PROGRESS ",
                    onProgress: onProgress
                )
                do {
                    let result = try runProcessCollectingStreamingOutput(
                        process,
                        outputPipe: outputPipe,
                        errorPipe: errorPipe,
                        collector: collector
                    )
                    let text = String(decoding: result.data, as: UTF8.self)
                    if result.terminationStatus == 0 { continuation.resume(returning: text.trimmingCharacters(in: .whitespacesAndNewlines)) }
                    else { continuation.resume(throwing: GatewayError.command(text.isEmpty ? "exit \(result.terminationStatus)" : text)) }
                } catch { continuation.resume(throwing: error) }
            }
        }
    }

    func gatewayExecutableURL() throws -> URL {
        if let resourceURL = Bundle.main.resourceURL {
            let bundled = resourceURL.appendingPathComponent("codex-mixin")
            if FileManager.default.isExecutableFile(atPath: bundled.path) {
                return bundled
            }
        }
        throw GatewayError.command("bundled codex-mixin executable not found")
    }

    func stateDir() -> URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".codex-mixin")
    }

    func appendDiagnosticLog(_ message: String) {
        appendAppDiagnosticLog(message, directory: stateDir())
    }

    func launchAgentPath() -> URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents")
            .appendingPathComponent("\(serviceLabel).plist")
    }

    func menuLaunchAgentPath() -> URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents")
            .appendingPathComponent("\(menuLaunchLabel).plist")
    }

    func launchDomain() -> String {
        "gui/\(getuid())"
    }

    func launchDomainAndLabel() -> String {
        "\(launchDomain())/\(serviceLabel)"
    }

    func menuLaunchDomainAndLabel() -> String {
        "\(launchDomain())/\(menuLaunchLabel)"
    }
}
