import Foundation
import Darwin

private let appDiagnosticLogQueue = DispatchQueue(
    label: "local.codex-mixin.app-diagnostic-log"
)

private let diagnosticSensitiveOptions: Set<String> = [
    "--key",
    "--api-key",
    "--gateway-key",
    "--token",
    "--access-token",
    "--refresh-token",
    "--secret",
    "--password",
]

func diagnosticCommandDescription(_ arguments: [String]) -> String {
    var rendered: [String] = []
    var redactNext = false
    for argument in arguments {
        if redactNext {
            rendered.append("<redacted>")
            redactNext = false
            continue
        }
        if diagnosticSensitiveOptions.contains(argument) {
            rendered.append(argument)
            redactNext = true
            continue
        }
        if let separator = argument.firstIndex(of: "=") {
            let option = String(argument[..<separator])
            if diagnosticSensitiveOptions.contains(option) {
                rendered.append("\(option)=<redacted>")
                continue
            }
        }
        rendered.append(quoteDiagnosticArgument(argument))
    }
    return diagnosticSafeText(rendered.joined(separator: " "))
}

func diagnosticOutputSummary(arguments: [String], output: String) -> String {
    let outputBytes = output.lengthOfBytes(using: .utf8)
    let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else {
        return "output_bytes=\(outputBytes)"
    }
    guard shouldIncludeDiagnosticOutput(arguments) else {
        return "output_bytes=\(outputBytes) content=omitted"
    }
    return "output_bytes=\(outputBytes)\noutput:\n\(diagnosticSafeText(trimmed).prefix(6_000))"
}

func diagnosticErrorDescription(_ error: Error) -> String {
    String(diagnosticSafeText(String(describing: error)).prefix(6_000))
}

func diagnosticSafeText(_ text: String) -> String {
    let patterns = [
        (
            #"(?i)(--(?:api-key|gateway-key|key|token|access-token|refresh-token|secret|password))(?:=|\s+)(?:"[^"]*"|'[^']*'|[^\s,;]+)"#,
            "$1=<redacted>"
        ),
        (
            #"(?i)("(?:api[_-]?key|gateway[_-]?key|access[_-]?token|refresh[_-]?token|token|secret|password|authorization)"\s*:\s*)(?:"[^"]*"|[^,}\s]+)"#,
            "$1\"<redacted>\""
        ),
        (
            #"(?i)(authorization\s*:\s*bearer\s+)[^\s,;]+"#,
            "$1<redacted>"
        ),
        (
            #"(?im)\b(api[_-]?key|gateway[_-]?key|access[_-]?token|refresh[_-]?token|token|secret|password|authorization)\s*([=:])\s*(?:"[^"]*"|'[^']*'|[^\s,;]+)"#,
            "$1$2<redacted>"
        ),
    ]
    return patterns.reduce(text) { result, pattern in
        guard let expression = try? NSRegularExpression(
            pattern: pattern.0,
            options: []
        ) else {
            return result
        }
        let range = NSRange(result.startIndex..<result.endIndex, in: result)
        return expression.stringByReplacingMatches(
            in: result,
            options: [],
            range: range,
            withTemplate: pattern.1
        )
    }
}

func appendAppDiagnosticLog(_ message: String, directory: URL) {
    appDiagnosticLogQueue.sync {
        let logURL = directory.appendingPathComponent("gateway.log")
        do {
            try FileManager.default.createDirectory(
                at: directory,
                withIntermediateDirectories: true
            )
            let descriptor = open(
                logURL.path,
                O_WRONLY | O_CREAT | O_APPEND,
                S_IRUSR | S_IWUSR
            )
            guard descriptor >= 0 else {
                throw NSError(
                    domain: NSPOSIXErrorDomain,
                    code: Int(errno),
                    userInfo: [NSLocalizedDescriptionKey: String(cString: strerror(errno))]
                )
            }
            defer { close(descriptor) }
            guard fchmod(descriptor, S_IRUSR | S_IWUSR) == 0 else {
                throw NSError(
                    domain: NSPOSIXErrorDomain,
                    code: Int(errno),
                    userInfo: [NSLocalizedDescriptionKey: String(cString: strerror(errno))]
                )
            }

            let formatter = ISO8601DateFormatter()
            let boundedMessage = String(diagnosticSafeText(message).prefix(8_000))
            let entry = "\n\(formatter.string(from: Date())) APP_DIAGNOSTIC \(boundedMessage)\n"
            let data = Data(entry.utf8)
            try data.withUnsafeBytes { rawBuffer in
                guard let baseAddress = rawBuffer.baseAddress else { return }
                var written = 0
                while written < rawBuffer.count {
                    let result = Darwin.write(
                        descriptor,
                        baseAddress.advanced(by: written),
                        rawBuffer.count - written
                    )
                    guard result > 0 else {
                        throw NSError(
                            domain: NSPOSIXErrorDomain,
                            code: Int(errno),
                            userInfo: [
                                NSLocalizedDescriptionKey: String(cString: strerror(errno)),
                            ]
                        )
                    }
                    written += result
                }
            }
        } catch {
            NSLog("Codex Mixin could not append diagnostic log: \(error)")
        }
    }
}

private func shouldIncludeDiagnosticOutput(_ arguments: [String]) -> Bool {
    guard let command = arguments.first else { return false }
    if [
        "install-codex",
        "uninstall-codex",
        "refresh-codex-catalog",
        "doctor",
    ].contains(command) {
        return true
    }
    if command == "providers", arguments.count > 1 {
        return arguments[1] != "list"
    }
    return false
}

private func quoteDiagnosticArgument(_ argument: String) -> String {
    guard argument.contains(where: \.isWhitespace) || argument.contains("\"") else {
        return argument
    }
    return "\"\(argument.replacingOccurrences(of: "\"", with: "\\\""))\""
}
