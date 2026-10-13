import Foundation

@main
struct LaunchAgentBootstrapTests {
    static func main() {
        let menuPlist = menuLaunchAgentPlist(
            label: "local.codex-mixin.menu-launch",
            executablePath: "/Applications/Codex Mixin & Tools.app/Contents/MacOS/CodexMixinMenu",
            logPath: "/Users/test/.codex-mixin/macos-app.log"
        )
        precondition(menuPlist.contains("Codex Mixin &amp; Tools.app"))
        precondition(!menuPlist.contains("<string>/usr/bin/open</string>"))
        precondition(menuPlist.contains("<key>KeepAlive</key>"))
        precondition(menuPlist.contains("<key>SuccessfulExit</key>"))
        precondition(menuPlist.contains("macos-app.log"))

        testDefaultAppliesOnce()
        testSystemDisableWins()
        testPlatformAndBundlePathLimits()
        testDisabledOverrideParsing()

        print("GUI login launch agent plist: passed")
    }

    private static func baseState() -> LaunchAtLoginState {
        LaunchAtLoginState(
            platformSupportsStartupService: true,
            bundlePathIsStable: true,
            launchAgentInstalled: false,
            systemDisabled: false,
            defaultAppliedMarkerExists: false
        )
    }

    private static func testDefaultAppliesOnce() {
        precondition(launchAtLoginAction(for: baseState()) == .installDefault)
        var installed = baseState()
        installed.launchAgentInstalled = true
        precondition(
            launchAtLoginAction(for: installed) == .keepAsIs(.agentInstalled)
        )
        var applied = baseState()
        applied.defaultAppliedMarkerExists = true
        precondition(
            launchAtLoginAction(for: applied) == .keepAsIs(.defaultAlreadyApplied),
            "a removed login item must stay removed"
        )
        var both = baseState()
        both.launchAgentInstalled = true
        both.defaultAppliedMarkerExists = true
        precondition(
            launchAtLoginAction(for: both) == .keepAsIs(.agentInstalled)
        )
    }

    private static func testSystemDisableWins() {
        var disabled = baseState()
        disabled.systemDisabled = true
        precondition(
            launchAtLoginAction(for: disabled) == .keepAsIs(.systemDisabled),
            "an explicit system disable must never be overridden"
        )
        var disabledAndInstalled = baseState()
        disabledAndInstalled.systemDisabled = true
        disabledAndInstalled.launchAgentInstalled = true
        precondition(
            launchAtLoginAction(for: disabledAndInstalled) == .keepAsIs(.agentInstalled)
        )
    }

    private static func testPlatformAndBundlePathLimits() {
        var unsupported = baseState()
        unsupported.platformSupportsStartupService = false
        precondition(
            launchAtLoginAction(for: unsupported) == .keepAsIs(.platformUnsupported)
        )
        var unstable = baseState()
        unstable.bundlePathIsStable = false
        precondition(
            launchAtLoginAction(for: unstable) == .keepAsIs(.unstableBundlePath)
        )
        precondition(
            launchAtLoginBundlePathIsStable("/Applications/Codex Mixin.app")
        )
        precondition(
            !launchAtLoginBundlePathIsStable(
                "/private/var/folders/xy/AppTranslocation/1234/d/Codex Mixin.app"
            )
        )
        precondition(
            !launchAtLoginBundlePathIsStable("/Volumes/Codex Mixin/Codex Mixin.app")
        )
        precondition(!launchAtLoginBundlePathIsStable(""))
        precondition(launchAtLoginSkipDescription(.installDefault) == "install default")
        precondition(
            launchAtLoginSkipDescription(.keepAsIs(.systemDisabled))
                == "the system disabled the launch agent"
        )
    }

    private static func testDisabledOverrideParsing() {
        let output = """
        disabled services = {
        \t"com.apple.other" => true
        \t"local.codex-mixin.menu-launch" => true
        }
        """
        precondition(
            launchAgentDisabledOverride(in: output, label: "local.codex-mixin.menu-launch")
        )
        let enabled = """
        disabled services = {
        \t"local.codex-mixin.menu-launch" => false
        }
        """
        precondition(
            !launchAgentDisabledOverride(in: enabled, label: "local.codex-mixin.menu-launch")
        )
        precondition(
            !launchAgentDisabledOverride(in: "disabled services = {}", label: "local.codex-mixin.menu-launch")
        )
        precondition(
            !launchAgentDisabledOverride(in: output, label: "local.codex-mixin.other")
        )
    }
}
