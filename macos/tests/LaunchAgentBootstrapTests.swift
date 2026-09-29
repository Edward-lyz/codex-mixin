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
        print("GUI login launch agent plist: passed")
    }
}
