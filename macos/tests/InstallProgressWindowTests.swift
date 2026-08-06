import Cocoa

@main
struct InstallProgressWindowTests {
    static func main() {
        _ = NSApplication.shared

        precondition(
            localizedProgressLabel("Fetching available models") == "获取可用模型列表"
        )
        precondition(
            localizedProgressLabel("检查本地配置与网关状态") == "检查本地配置与网关状态"
        )
        precondition(
            localizedProgressLabel("unknown progress line") == "unknown progress line"
        )

        let controller = InstallProgressWindowController(
            title: "测试进度",
            detail: "detail",
            successTitle: "✓ done",
            failureTitle: "✗ fail"
        )
        controller.setPhases([
            "第一步",
            "第二步",
            "第三步",
        ])
        controller.present()
        controller.advance(to: 0)
        controller.advanceStreamedPhase("Fetching available models")
        controller.advanceStreamedPhase("Loading model metadata")
        controller.advanceStreamedPhase("Writing Codex config and model catalog")
        // finish() must keep the controller alive long enough to auto-close.
        var retained: InstallProgressWindowController? = controller
        retained?.finish()
        RunLoop.current.run(until: Date().addingTimeInterval(1.0))
        precondition(retained?.window?.isVisible != true, "success progress window must auto-close")
        retained = nil

        let failing = InstallProgressWindowController(
            title: "失败路径",
            successTitle: "✓ done",
            failureTitle: "✗ fail"
        )
        failing.setPhases(["准备", "执行"])
        failing.present()
        failing.fail(message: "boom")
        precondition(failing.window?.styleMask.contains(.closable) == true)

        print("Install progress window tests passed")
    }
}
