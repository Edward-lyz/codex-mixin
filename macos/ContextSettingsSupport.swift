import Foundation

/// Global context-engine settings reported by `context config get --json`.
///
/// Only the `context` object is consumed: the CLI owns `config.json` and
/// `context.db`, and the app never reads or writes those files.
struct ContextSettings: Equatable {
    var enabled: Bool
    var growthThresholdTokens: Int
    var pressureRatio: Double
    var urgentRatio: Double
    var protectedMessages: Int
    var protectedTokens: Int
    var maxInternalRounds: Int
    /// `nil` follows the gateway or model default window.
    var contextWindowTokens: Int?

    static func fromCLIJSON(_ rawJSON: String) throws -> ContextSettings {
        guard
            let root = try? JSONSerialization.jsonObject(with: Data(rawJSON.utf8)),
            let response = root as? [String: Any],
            let context = response["context"] as? [String: Any]
        else {
            throw ContextSettingsError.message(
                AppLocalization.string("context.error.settingsJSON")
            )
        }
        guard let enabled = (context["enabled"] as? NSNumber)?.boolValue else {
            throw ContextSettingsError.missingField("enabled")
        }
        func integer(_ key: String) throws -> Int {
            guard let number = context[key] as? NSNumber else {
                throw ContextSettingsError.missingField(key)
            }
            return number.intValue
        }
        func ratio(_ key: String) throws -> Double {
            guard let number = context[key] as? NSNumber else {
                throw ContextSettingsError.missingField(key)
            }
            return number.doubleValue
        }
        return ContextSettings(
            enabled: enabled,
            growthThresholdTokens: try integer("growth_threshold_tokens"),
            pressureRatio: try ratio("pressure_ratio"),
            urgentRatio: try ratio("urgent_ratio"),
            protectedMessages: try integer("protected_messages"),
            protectedTokens: try integer("protected_tokens"),
            maxInternalRounds: try integer("max_internal_rounds"),
            // `null` and a missing key both mean "follow the model".
            contextWindowTokens: (context["context_window_tokens"] as? NSNumber)?.intValue
        )
    }
}

/// The first field that blocks a save. The view maps each case to a localized
/// message, so this file stays free of UI text for validated input.
enum ContextSettingsValidationIssue: Error, Equatable {
    case growthThreshold
    case pressureRatio
    case urgentRatio
    case ratioOrder
    case protectedMessages
    case protectedTokens
    case maxInternalRounds
    case contextWindowTokens
}

/// Editable text for the settings form. Text is kept as typed so users can
/// clear or retype a field; `validated()` converts it to CLI values.
struct ContextSettingsDraft: Equatable {
    var enabled: Bool
    var growthThresholdTokens: String
    var pressureRatio: String
    var urgentRatio: String
    var protectedMessages: String
    var protectedTokens: String
    var maxInternalRounds: String
    var followsModelWindow: Bool
    var contextWindowTokens: String

    init() {
        enabled = false
        growthThresholdTokens = ""
        pressureRatio = ""
        urgentRatio = ""
        protectedMessages = ""
        protectedTokens = ""
        maxInternalRounds = ""
        followsModelWindow = true
        contextWindowTokens = ""
    }

    init(settings: ContextSettings) {
        enabled = settings.enabled
        growthThresholdTokens = String(settings.growthThresholdTokens)
        pressureRatio = String(settings.pressureRatio)
        urgentRatio = String(settings.urgentRatio)
        protectedMessages = String(settings.protectedMessages)
        protectedTokens = String(settings.protectedTokens)
        maxInternalRounds = String(settings.maxInternalRounds)
        followsModelWindow = settings.contextWindowTokens == nil
        contextWindowTokens = settings.contextWindowTokens.map(String.init) ?? ""
    }

    /// Mirrors `ContextSettings::validate` in the Rust CLI so the form reports
    /// invalid input before spawning a process.
    func validated() -> Result<ContextSettings, ContextSettingsValidationIssue> {
        guard let growth = Int(value(growthThresholdTokens)), growth > 0 else {
            return .failure(.growthThreshold)
        }
        guard let pressure = Double(value(pressureRatio)), pressure > 0, pressure <= 1 else {
            return .failure(.pressureRatio)
        }
        guard let urgent = Double(value(urgentRatio)), urgent > 0, urgent <= 1 else {
            return .failure(.urgentRatio)
        }
        guard pressure < urgent else {
            return .failure(.ratioOrder)
        }
        guard let messages = Int(value(protectedMessages)), messages > 0 else {
            return .failure(.protectedMessages)
        }
        guard let tokens = Int(value(protectedTokens)), tokens > 0 else {
            return .failure(.protectedTokens)
        }
        guard let rounds = Int(value(maxInternalRounds)), (1...8).contains(rounds) else {
            return .failure(.maxInternalRounds)
        }
        var window: Int?
        if !followsModelWindow {
            guard let windowTokens = Int(value(contextWindowTokens)), windowTokens > 0 else {
                return .failure(.contextWindowTokens)
            }
            window = windowTokens
        }
        return .success(ContextSettings(
            enabled: enabled,
            growthThresholdTokens: growth,
            pressureRatio: pressure,
            urgentRatio: urgent,
            protectedMessages: messages,
            protectedTokens: tokens,
            maxInternalRounds: rounds,
            contextWindowTokens: window
        ))
    }

    private func value(_ text: String) -> String {
        text.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

/// Builds `context config set` arguments that carry only the changed fields.
/// An empty result means there is nothing to save.
func contextSettingsArguments(from saved: ContextSettings, to edited: ContextSettings) -> [String] {
    var arguments = ["context", "config", "set"]
    if saved.enabled != edited.enabled {
        arguments += ["--enabled", edited.enabled ? "true" : "false"]
    }
    if saved.growthThresholdTokens != edited.growthThresholdTokens {
        arguments += ["--growth-threshold-tokens", String(edited.growthThresholdTokens)]
    }
    if saved.pressureRatio != edited.pressureRatio {
        arguments += ["--pressure-ratio", String(edited.pressureRatio)]
    }
    if saved.urgentRatio != edited.urgentRatio {
        arguments += ["--urgent-ratio", String(edited.urgentRatio)]
    }
    if saved.protectedMessages != edited.protectedMessages {
        arguments += ["--protected-messages", String(edited.protectedMessages)]
    }
    if saved.protectedTokens != edited.protectedTokens {
        arguments += ["--protected-tokens", String(edited.protectedTokens)]
    }
    if saved.maxInternalRounds != edited.maxInternalRounds {
        arguments += ["--max-internal-rounds", String(edited.maxInternalRounds)]
    }
    if saved.contextWindowTokens != edited.contextWindowTokens {
        arguments += [
            "--context-window-tokens",
            edited.contextWindowTokens.map(String.init) ?? "none",
        ]
    }
    guard arguments.count > 3 else { return [] }
    return arguments + ["--json", "--json-errors"]
}

// MARK: - Statistics

struct ContextCacheStats: Decodable, Equatable {
    let turns: Int
    let inputTokens: UInt64
    let cachedTokens: UInt64
    let outputTokens: UInt64
    let hitPercent: Double

    private enum CodingKeys: String, CodingKey {
        case turns
        case inputTokens = "input_tokens"
        case cachedTokens = "cached_tokens"
        case outputTokens = "output_tokens"
        case hitPercent = "hit_percent"
    }
}

struct ContextSessionStats: Decodable, Identifiable, Equatable {
    let session: String
    let display: String
    let compressions: Int
    let savedTokens: UInt64
    let cache: ContextCacheStats

    var id: String { session }

    private enum CodingKeys: String, CodingKey {
        case session
        case display
        case compressions
        case savedTokens = "saved_tokens"
        case cache
    }
}

struct ContextStatsTotals: Decodable, Equatable {
    let sessions: Int
    let compressions: Int
    let savedTokens: UInt64
    let cache: ContextCacheStats

    private enum CodingKeys: String, CodingKey {
        case sessions
        case compressions
        case savedTokens = "saved_tokens"
        case cache
    }
}

/// Decoded `context stats --json`. Fields the page does not show (tiers,
/// revision, view tokens) are ignored by `Decodable`.
struct ContextStats: Decodable, Equatable {
    let enabled: Bool
    let totals: ContextStatsTotals
    let sessions: [ContextSessionStats]
}

func parseContextSettings(_ rawJSON: String) throws -> ContextSettings {
    try ContextSettings.fromCLIJSON(rawJSON)
}

func parseContextStats(_ rawJSON: String) throws -> ContextStats {
    do {
        return try JSONDecoder().decode(ContextStats.self, from: Data(rawJSON.utf8))
    } catch {
        throw ContextSettingsError.message(
            AppLocalization.string("context.error.statsJSON", String(describing: error))
        )
    }
}

/// Share of billed input tokens served from the upstream cache.
func formatContextPercent(_ percent: Double) -> String {
    String(format: "%.1f%%", percent)
}

// MARK: - CLI errors

/// The `--json-errors` envelope of one failed CLI call.
struct ContextCLIErrorInfo: Equatable {
    let code: String
    let message: String
    /// `nil` means the CLI did not report whether the update was written.
    let committed: Bool?
    let stage: String?
}

/// Finds the JSON error envelope on the final stderr line. The gateway
/// boundary joins stderr and stdout, so scan every line from the end.
func parseContextCLIError(_ raw: String) -> ContextCLIErrorInfo? {
    for line in raw.split(separator: "\n", omittingEmptySubsequences: true).reversed() {
        guard
            let root = try? JSONSerialization.jsonObject(with: Data(line.utf8)),
            let response = root as? [String: Any],
            (response["protocol_version"] as? NSNumber)?.intValue == 1,
            let error = response["error"] as? [String: Any],
            let code = error["code"] as? String,
            let message = error["message"] as? String
        else { continue }
        return ContextCLIErrorInfo(
            code: code,
            message: message,
            committed: (error["committed"] as? NSNumber)?.boolValue,
            stage: error["stage"] as? String
        )
    }
    return nil
}

enum ContextSettingsError: Error, Equatable, CustomStringConvertible {
    case message(String)
    case missingField(String)
    case cli(ContextCLIErrorInfo)

    var description: String {
        switch self {
        case .message(let message):
            return message
        case .missingField(let field):
            return AppLocalization.string("context.error.missingField", field)
        case .cli(let info):
            return info.message
        }
    }

    /// Converts a gateway process failure into the structured CLI error when
    /// the failed call emitted one; otherwise keeps the raw output.
    static func from(_ error: Error) -> ContextSettingsError {
        if let error = error as? ContextSettingsError { return error }
        let raw = String(describing: error)
        if let info = parseContextCLIError(raw) { return .cli(info) }
        return .message(raw)
    }
}
