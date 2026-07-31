import Cocoa

private final class AboutActionTarget: NSObject {
    let action: () -> Void

    init(_ action: @escaping () -> Void) {
        self.action = action
    }

    @objc func run(_ sender: Any?) {
        action()
    }
}

struct AppAboutInfo: Equatable {
    let version: String
    let build: String
    let repositoryURL: String

    static var current: AppAboutInfo {
        let bundle = Bundle.main
        return AppAboutInfo(
            version: bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.0.0",
            build: bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "0.0.0",
            repositoryURL: "https://github.com/Edward-lyz/codex-mixin"
        )
    }
}

final class AboutWindowController: NSWindowController, NSWindowDelegate {
    private let info: AppAboutInfo

    init(info: AppAboutInfo = .current) {
        self.info = info
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 560, height: 420),
            styleMask: [.titled, .closable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = appText("关于 Codex Mixin", "關於 Codex Mixin", "About Codex Mixin")
        window.minSize = NSSize(width: 520, height: 380)
        window.isReleasedWhenClosed = false
        window.center()
        super.init(window: window)
        window.delegate = self
        buildContent(in: window)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() {
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func buildContent(in window: NSWindow) {
        guard let contentView = window.contentView else { return }

        let iconView = NSImageView()
        iconView.image = NSImage(named: "CodexMixin") ?? NSApplication.shared.applicationIconImage
        iconView.imageScaling = .scaleProportionallyUpOrDown
        iconView.translatesAutoresizingMaskIntoConstraints = false
        iconView.widthAnchor.constraint(equalToConstant: 96).isActive = true
        iconView.heightAnchor.constraint(equalToConstant: 96).isActive = true

        let nameLabel = NSTextField(labelWithString: "Codex Mixin")
        nameLabel.font = .systemFont(ofSize: 26, weight: .bold)
        nameLabel.alignment = .center

        let tagline = NSTextField(wrappingLabelWithString: appText(
            "把自定义模型接进官方 Codex，同时保留 ChatGPT 账号、官方 GPT 模型与原生体验。",
            "把自訂模型接進官方 Codex，同時保留 ChatGPT 帳號、官方 GPT 模型與原生體驗。",
            "Bring custom model providers into official Codex while keeping ChatGPT account features, official GPT models, and the native experience."
        ))
        tagline.font = .systemFont(ofSize: 13)
        tagline.textColor = .secondaryLabelColor
        tagline.alignment = .center
        tagline.preferredMaxLayoutWidth = 420

        let versionLabel = NSTextField(labelWithString: appText(
            "版本 \(info.version)",
            "版本 \(info.version)",
            "Version \(info.version)"
        ))
        versionLabel.font = .monospacedSystemFont(ofSize: 12, weight: .medium)
        versionLabel.textColor = .secondaryLabelColor
        versionLabel.alignment = .center
        versionLabel.lineBreakMode = .byTruncatingMiddle

        let buildLabel = NSTextField(labelWithString: appText(
            "Build \(info.build)",
            "Build \(info.build)",
            "Build \(info.build)"
        ))
        buildLabel.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        buildLabel.textColor = .tertiaryLabelColor
        buildLabel.alignment = .center

        let repositoryButton = NSButton(
            title: appText("GitHub 仓库", "GitHub 儲存庫", "GitHub Repository"),
            target: nil,
            action: nil
        )
        repositoryButton.bezelStyle = .rounded
        repositoryButton.image = menuItemImage("link")
        repositoryButton.imagePosition = .imageLeading
        repositoryButton.toolTip = info.repositoryURL
        let repositoryTarget = AboutActionTarget { [repositoryURL = info.repositoryURL] in
            if let url = URL(string: repositoryURL) {
                NSWorkspace.shared.open(url)
            }
        }
        repositoryButton.target = repositoryTarget
        repositoryButton.action = #selector(AboutActionTarget.run(_:))
        repositoryButton.translatesAutoresizingMaskIntoConstraints = false
        repositoryButton.widthAnchor.constraint(greaterThanOrEqualToConstant: 140).isActive = true

        let copyButton = NSButton(
            title: appText("复制版本信息", "複製版本資訊", "Copy Version Info"),
            target: nil,
            action: nil
        )
        copyButton.bezelStyle = .rounded
        copyButton.image = menuItemImage("doc.on.doc")
        copyButton.imagePosition = .imageLeading
        let copyTarget = AboutActionTarget { [info] in
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(
                "Codex Mixin \(info.version) (\(info.build))\n\(info.repositoryURL)",
                forType: .string
            )
        }
        copyButton.target = copyTarget
        copyButton.action = #selector(AboutActionTarget.run(_:))
        copyButton.translatesAutoresizingMaskIntoConstraints = false
        copyButton.widthAnchor.constraint(greaterThanOrEqualToConstant: 140).isActive = true

        let buttons = NSStackView(views: [repositoryButton, copyButton])
        buttons.orientation = .horizontal
        buttons.alignment = .centerY
        buttons.spacing = 12

        let center = NSStackView(views: [
            iconView,
            nameLabel,
            tagline,
            versionLabel,
            buildLabel,
            buttons,
        ])
        center.orientation = .vertical
        center.alignment = .centerX
        center.spacing = 9
        center.translatesAutoresizingMaskIntoConstraints = false

        contentView.addSubview(center)
        NSLayoutConstraint.activate([
            center.centerXAnchor.constraint(equalTo: contentView.centerXAnchor),
            center.centerYAnchor.constraint(equalTo: contentView.centerYAnchor),
            center.leadingAnchor.constraint(greaterThanOrEqualTo: contentView.leadingAnchor, constant: 36),
            center.trailingAnchor.constraint(lessThanOrEqualTo: contentView.trailingAnchor, constant: -36),
            iconView.widthAnchor.constraint(equalToConstant: 96),
            iconView.heightAnchor.constraint(equalToConstant: 96),
            tagline.widthAnchor.constraint(equalTo: contentView.widthAnchor, constant: -96),
            buttons.heightAnchor.constraint(equalToConstant: 34),
        ])
    }
}
