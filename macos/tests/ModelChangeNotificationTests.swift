import Foundation

@main
struct ModelChangeNotificationTests {
    static func main() {
        let payload = ModelChangeNotificationPayload(arguments: [
            "/Applications/Codex Mixin.app/Contents/MacOS/CodexMixinMenu",
            "--deliver-model-notification",
            "Codex Mixin",
            "我的常用模型 · 模型列表已更新",
            "✓ 新增并探测 2 个：gpt-5.6-luna、gpt-6-astra\n− 下线并移除 1 个：gpt-image-2",
        ])
        precondition(payload?.title == "Codex Mixin")
        precondition(payload?.subtitle == "我的常用模型 · 模型列表已更新")
        precondition(payload?.body.contains("\n") == true)
        precondition(
            ModelChangeNotificationPayload(arguments: ["CodexMixinMenu"]) == nil,
            "normal app launches must not be interpreted as notification delivery"
        )
        precondition(
            ModelChangeNotificationPayload(arguments: [
                "CodexMixinMenu",
                "--deliver-model-notification",
                "Codex Mixin",
            ]) == nil,
            "incomplete notification payloads must fail fast"
        )

        print("Model change notification payload: passed")
    }
}
