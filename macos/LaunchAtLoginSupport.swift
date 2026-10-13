import Foundation

/// Why the app left the launch-at-login setup alone.
enum LaunchAtLoginSkipReason: Equatable {
    case platformUnsupported
    case unstableBundlePath
    case agentInstalled
    case systemDisabled
    case defaultAlreadyApplied
}

enum LaunchAtLoginAction: Equatable {
    case installDefault
    case keepAsIs(LaunchAtLoginSkipReason)
}

struct LaunchAtLoginState: Equatable {
    var platformSupportsStartupService: Bool
    var bundlePathIsStable: Bool
    var launchAgentInstalled: Bool
    var systemDisabled: Bool
    var defaultAppliedMarkerExists: Bool
}

/// Launch at login is on by default, but the app applies that default once:
/// a system-level disable or a login item the user removed stays removed, and
/// every later launch keeps the user's choice.
func launchAtLoginAction(for state: LaunchAtLoginState) -> LaunchAtLoginAction {
    guard state.platformSupportsStartupService else {
        return .keepAsIs(.platformUnsupported)
    }
    guard state.bundlePathIsStable else {
        return .keepAsIs(.unstableBundlePath)
    }
    if state.launchAgentInstalled {
        return .keepAsIs(.agentInstalled)
    }
    guard !state.systemDisabled else {
        return .keepAsIs(.systemDisabled)
    }
    guard !state.defaultAppliedMarkerExists else {
        return .keepAsIs(.defaultAlreadyApplied)
    }
    return .installDefault
}

/// A login agent must point at a path that survives the next login: App
/// Translocation, a mounted disk image, and temporary directories do not.
func launchAtLoginBundlePathIsStable(_ bundlePath: String) -> Bool {
    guard !bundlePath.isEmpty else { return false }
    let unstablePrefixes = [
        "/Volumes/",
        "/private/var/folders/",
        "/var/folders/",
    ]
    guard !unstablePrefixes.contains(where: { bundlePath.hasPrefix($0) }) else {
        return false
    }
    return !bundlePath.contains("/AppTranslocation/")
}

/// Reads `launchctl print-disabled` output, which lists overrides as
/// `"label" => true` lines.
func launchAgentDisabledOverride(in output: String, label: String) -> Bool {
    for rawLine in output.split(separator: "\n") {
        let line = rawLine.trimmingCharacters(in: .whitespaces)
        guard line.hasPrefix("\"\(label)\""),
              let separatorRange = line.range(of: "=>")
        else { continue }
        let value = line[separatorRange.upperBound...]
            .trimmingCharacters(in: .whitespaces)
        return value.hasPrefix("true")
    }
    return false
}

func launchAtLoginSkipDescription(_ action: LaunchAtLoginAction) -> String {
    switch action {
    case .installDefault:
        return "install default"
    case .keepAsIs(let reason):
        switch reason {
        case .platformUnsupported: return "platform does not support the startup service"
        case .unstableBundlePath: return "bundle path is not stable"
        case .agentInstalled: return "launch agent is already installed"
        case .systemDisabled: return "the system disabled the launch agent"
        case .defaultAlreadyApplied: return "the default was applied before"
        }
    }
}
