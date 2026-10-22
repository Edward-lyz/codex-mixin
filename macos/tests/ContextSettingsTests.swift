import Foundation

/// Focused checks for the context settings/stats boundary: CLI JSON decoding,
/// draft validation, the changed-fields argument diff, and error envelopes.
@main
struct ContextSettingsTests {
    static func main() throws {
        try checkSettingsDecoding()
        try checkSettingsValidation()
        checkArgumentDiff()
        try checkStatsDecoding()
        checkFormatting()
        try checkErrorEnvelope()
        print("Context settings logic: passed")
    }

    private static func checkSettingsDecoding() throws {
        // `config get` carries the paths; only `context` is consumed.
        let getJSON = """
        {
          "config_path": "/tmp/config.json",
          "context": {
            "context_window_tokens": null,
            "enabled": true,
            "growth_threshold_tokens": 50000,
            "max_internal_rounds": 4,
            "pressure_ratio": 0.75,
            "protected_messages": 5,
            "protected_tokens": 5000,
            "urgent_ratio": 0.95
          },
          "database_path": "/tmp/context.db"
        }
        """
        let decoded = try parseContextSettings(getJSON)
        precondition(decoded.enabled)
        precondition(decoded.growthThresholdTokens == 50000)
        precondition(decoded.pressureRatio == 0.75)
        precondition(decoded.urgentRatio == 0.95)
        precondition(decoded.protectedMessages == 5)
        precondition(decoded.protectedTokens == 5000)
        precondition(decoded.maxInternalRounds == 4)
        precondition(decoded.contextWindowTokens == nil)

        // `config set` returns only the `context` object.
        let setJSON = """
        {"context": {"context_window_tokens": 128000, "enabled": false,
          "growth_threshold_tokens": 40000, "max_internal_rounds": 2,
          "pressure_ratio": 0.6, "protected_messages": 3,
          "protected_tokens": 2000, "urgent_ratio": 0.9}}
        """
        let updated = try parseContextSettings(setJSON)
        precondition(updated.contextWindowTokens == 128000)
        precondition(!updated.enabled)

        // A malformed payload fails as a structured error, not a crash.
        do {
            _ = try parseContextSettings("not json")
            preconditionFailure("bad settings JSON should throw")
        } catch let error as ContextSettingsError {
            guard case .message = error else {
                preconditionFailure("bad settings JSON should be .message, got \(error)")
            }
        }
        do {
            _ = try parseContextSettings(#"{"context": {"enabled": true}}"#)
            preconditionFailure("missing fields should throw")
        } catch let error as ContextSettingsError {
            guard case .missingField = error else {
                preconditionFailure("missing field should be .missingField, got \(error)")
            }
        }
    }

    private static func checkSettingsValidation() throws {
        let base = ContextSettings(
            enabled: false,
            growthThresholdTokens: 50000,
            pressureRatio: 0.75,
            urgentRatio: 0.95,
            protectedMessages: 5,
            protectedTokens: 5000,
            maxInternalRounds: 4,
            contextWindowTokens: nil
        )
        // Round-trip keeps every value, including the empty window.
        let draft = ContextSettingsDraft(settings: base)
        guard case .success(let roundTripped) = draft.validated() else {
            preconditionFailure("a decoded settings value must validate")
        }
        precondition(roundTripped == base)
        precondition(draft.followsModelWindow)

        // Surrounding whitespace is trimmed instead of rejected.
        var spaced = draft
        spaced.growthThresholdTokens = " 60000 "
        guard case .success(let trimmed) = spaced.validated() else {
            preconditionFailure("trimmed numbers should validate")
        }
        precondition(trimmed.growthThresholdTokens == 60000)

        func issue(_ edit: (inout ContextSettingsDraft) -> Void) -> ContextSettingsValidationIssue {
            var draft = ContextSettingsDraft(settings: base)
            edit(&draft)
            guard case .failure(let issue) = draft.validated() else {
                preconditionFailure("expected validation failure")
            }
            return issue
        }
        precondition(issue { $0.growthThresholdTokens = "0" } == .growthThreshold)
        precondition(issue { $0.growthThresholdTokens = "abc" } == .growthThreshold)
        precondition(issue { $0.pressureRatio = "0" } == .pressureRatio)
        precondition(issue { $0.pressureRatio = "1.01" } == .pressureRatio)
        precondition(issue { $0.urgentRatio = "0" } == .urgentRatio)
        precondition(issue { $0.urgentRatio = "1.5" } == .urgentRatio)
        precondition(issue { $0.pressureRatio = "0.95" } == .ratioOrder)
        precondition(issue { $0.protectedMessages = "-1" } == .protectedMessages)
        precondition(issue { $0.protectedTokens = "0" } == .protectedTokens)
        precondition(issue { $0.maxInternalRounds = "0" } == .maxInternalRounds)
        precondition(issue { $0.maxInternalRounds = "11" } == .maxInternalRounds)
        precondition(issue {
            $0.followsModelWindow = false
            $0.contextWindowTokens = ""
        } == .contextWindowTokens)
        precondition(issue {
            $0.followsModelWindow = false
            $0.contextWindowTokens = "0"
        } == .contextWindowTokens)

        // A window typed while following the model is ignored, not sent.
        var followModel = ContextSettingsDraft(settings: base)
        followModel.contextWindowTokens = "128000"
        guard case .success(let followed) = followModel.validated() else {
            preconditionFailure("follow-model draft should validate")
        }
        precondition(followed.contextWindowTokens == nil)
    }

    private static func checkArgumentDiff() {
        let base = ContextSettings(
            enabled: false,
            growthThresholdTokens: 50000,
            pressureRatio: 0.75,
            urgentRatio: 0.95,
            protectedMessages: 5,
            protectedTokens: 5000,
            maxInternalRounds: 4,
            contextWindowTokens: nil
        )
        precondition(contextSettingsArguments(from: base, to: base).isEmpty)

        var enabledOnly = base
        enabledOnly.enabled = true
        precondition(
            contextSettingsArguments(from: base, to: enabledOnly)
                == ["context", "config", "set", "--enabled", "true", "--json", "--json-errors"]
        )

        var windowOnly = base
        windowOnly.contextWindowTokens = 200000
        precondition(
            contextSettingsArguments(from: base, to: windowOnly)
                == [
                    "context", "config", "set", "--context-window-tokens", "200000",
                    "--json", "--json-errors",
                ]
        )
        precondition(
            contextSettingsArguments(from: windowOnly, to: base)
                == [
                    "context", "config", "set", "--context-window-tokens", "none",
                    "--json", "--json-errors",
                ]
        )

        let allChanged = ContextSettings(
            enabled: true,
            growthThresholdTokens: 60000,
            pressureRatio: 0.6,
            urgentRatio: 0.9,
            protectedMessages: 3,
            protectedTokens: 2000,
            maxInternalRounds: 2,
            contextWindowTokens: 128000
        )
        let arguments = contextSettingsArguments(from: base, to: allChanged)
        precondition(Array(arguments.prefix(4)) == ["context", "config", "set", "--enabled"])
        precondition(arguments.contains("--growth-threshold-tokens"))
        precondition(arguments.contains("--protected-tokens"))
        precondition(arguments.last == "--json-errors")
    }

    private static func checkStatsDecoding() throws {
        // Extra fields the page ignores must not break decoding.
        let statsJSON = """
        {
          "database_path": "/tmp/context.db",
          "enabled": true,
          "sessions": [
            {
              "cache": {"cached_tokens": 250, "hit_percent": 62.5,
                "input_tokens": 400, "output_tokens": 90, "turns": 3},
              "compressions": 2,
              "display": "示例会话",
              "messages": 12,
              "revision": 3,
              "saved_tokens": 1500,
              "session": "session-a",
              "tiers": {"summary": 1},
              "updated_at": 1760000000,
              "view_tokens": 2200
            }
          ],
          "totals": {
            "cache": {"cached_tokens": 250, "hit_percent": 62.5,
              "input_tokens": 400, "output_tokens": 90, "turns": 3},
            "compressions": 2,
            "messages": 12,
            "saved_tokens": 1500,
            "sessions": 1,
            "view_tokens": 2200
          }
        }
        """
        let stats = try parseContextStats(statsJSON)
        precondition(stats.enabled)
        precondition(stats.totals.sessions == 1)
        precondition(stats.totals.compressions == 2)
        precondition(stats.totals.savedTokens == 1500)
        precondition(stats.totals.cache.hitPercent == 62.5)
        precondition(stats.sessions.count == 1)
        precondition(stats.sessions[0].session == "session-a")
        precondition(stats.sessions[0].display == "示例会话")
        precondition(stats.sessions[0].compressions == 2)
        precondition(stats.sessions[0].savedTokens == 1500)
        precondition(stats.sessions[0].cache.hitPercent == 62.5)
        precondition(stats.sessions[0].id == "session-a")

        // No archived sessions is a valid, empty result.
        let emptyJSON = """
        {"database_path": "/tmp/context.db", "enabled": false, "sessions": [],
         "totals": {"cache": {"cached_tokens": 0, "hit_percent": 0.0,
           "input_tokens": 0, "output_tokens": 0, "turns": 0},
           "compressions": 0, "messages": 0, "saved_tokens": 0,
           "sessions": 0, "view_tokens": 0}}
        """
        let empty = try parseContextStats(emptyJSON)
        precondition(empty.sessions.isEmpty)
        precondition(empty.totals.sessions == 0)

        do {
            _ = try parseContextStats("{}")
            preconditionFailure("stats without totals should throw")
        } catch let error as ContextSettingsError {
            guard case .message = error else {
                preconditionFailure("bad stats should be .message, got \(error)")
            }
        }
    }

    private static func checkFormatting() {
        precondition(formatContextPercent(0) == "0.0%")
        precondition(formatContextPercent(100) == "100.0%")
        precondition(formatContextPercent(62.36) == "62.4%")
    }

    private static func checkErrorEnvelope() throws {
        let raw = """
        warning: something on stderr
        {"error":{"code":"operation_failed","committed":null,"message":"update context settings: context pressure_ratio must be in (0, 1]","stage":null},"protocol_version":1}
        """
        guard let info = parseContextCLIError(raw) else {
            preconditionFailure("error envelope should parse")
        }
        precondition(info.code == "operation_failed")
        precondition(info.message == "update context settings: context pressure_ratio must be in (0, 1]")
        precondition(info.committed == nil)
        precondition(info.stage == nil)

        let committedFalse = """
        {"error":{"code":"operation_failed","committed":false,"message":"boom","stage":"write"},"protocol_version":1}
        """
        let second = parseContextCLIError(committedFalse)
        precondition(second?.committed == false)
        precondition(second?.stage == "write")

        precondition(parseContextCLIError("plain failure text") == nil)

        struct RawFailure: Error, CustomStringConvertible {
            let description: String
        }
        let converted = ContextSettingsError.from(RawFailure(description: raw))
        guard case .cli(let fromEnvelope) = converted else {
            preconditionFailure("raw output with an envelope should become .cli")
        }
        precondition(fromEnvelope.message == info.message)

        let plain = ContextSettingsError.from(RawFailure(description: "plain failure text"))
        precondition(plain == .message("plain failure text"))
        precondition(ContextSettingsError.from(plain) == plain)
        precondition(
            ContextSettingsError.missingField("pressure_ratio").description
                == AppLocalization.string("context.error.missingField", "pressure_ratio")
        )
    }
}
