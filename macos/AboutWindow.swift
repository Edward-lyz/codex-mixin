import Cocoa
import SwiftUI

private final class AboutContentView: NSView {
    override var wantsUpdateLayer: Bool { true }

    override func updateLayer() {
        layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
    }
}

struct AppAboutInfo: Equatable {
    let version: String
    let build: String
    let repositoryURL: String

    var versionSummary: String {
        "Codex Mixin \(version) (\(build))\n\(repositoryURL)"
    }

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
    typealias OpenURLHandler = (URL) -> Void
    typealias CopyTextHandler = (String) -> Void
    typealias ShowCardHandler = (_ wallpaperOffset: Int) -> Void

    private let info: AppAboutInfo
    private let cardIdentity: CardIdentityV1
    private let wallpaperOffset: Int
    private let openURL: OpenURLHandler
    private let copyText: CopyTextHandler
    private let showCard: ShowCardHandler
    private let copyButton = NSButton()

    init(
        info: AppAboutInfo = .current,
        cardIdentity: CardIdentityV1 = CardIdentityStore.standard.current(),
        wallpaperOffset: Int = CardWallpaperSelectionStore.standard.nextOffset(
            count: CardWallpaperCatalog.wallpapers.count
        ),
        openURL: @escaping OpenURLHandler = { NSWorkspace.shared.open($0) },
        copyText: @escaping CopyTextHandler = { text in
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
        },
        showCard: @escaping ShowCardHandler = { _ in }
    ) {
        self.info = info
        self.cardIdentity = cardIdentity
        self.wallpaperOffset = wallpaperOffset
        self.openURL = openURL
        self.copyText = copyText
        self.showCard = showCard

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 820, height: 460),
            styleMask: [.titled, .closable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = L10n.About.title
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.isMovableByWindowBackground = true
        window.isReleasedWhenClosed = false
        window.contentView = AboutContentView(frame: window.contentView?.bounds ?? .zero)
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

    @objc private func openRepository(_ sender: Any?) {
        guard let url = URL(string: info.repositoryURL) else { return }
        openURL(url)
    }

    @objc private func copyVersionInfo(_ sender: Any?) {
        copyText(info.versionSummary)
        copyButton.title = L10n.About.copied
        copyButton.image = menuItemImage("checkmark")

        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
            self?.configureCopyButton()
        }
    }

    private func buildContent(in window: NSWindow) {
        guard let contentView = window.contentView else { return }

        let brandPanel = NSVisualEffectView()
        brandPanel.material = .sidebar
        brandPanel.blendingMode = .withinWindow
        brandPanel.state = .active
        brandPanel.translatesAutoresizingMaskIntoConstraints = false

        let cardPreview = NSHostingView(rootView: InstallCardThumbnailView(
            identity: cardIdentity,
            wallpaperOffset: wallpaperOffset,
            onOpen: { [showCard, wallpaperOffset] in showCard(wallpaperOffset) }
        ))
        cardPreview.identifier = NSUserInterfaceItemIdentifier("about.card-preview")
        cardPreview.translatesAutoresizingMaskIntoConstraints = false

        let cardHint = NSTextField(labelWithString: L10n.About.cardHint)
        cardHint.font = .systemFont(ofSize: 11, weight: .medium)
        cardHint.textColor = .secondaryLabelColor
        cardHint.alignment = .center

        let versionLabel = NSTextField(labelWithString: L10n.About.version(info.version))
        versionLabel.font = .monospacedSystemFont(ofSize: 12, weight: .medium)
        versionLabel.textColor = .secondaryLabelColor
        versionLabel.alignment = .center

        let buildLabel = NSTextField(labelWithString: "Build \(info.build)")
        buildLabel.font = .monospacedSystemFont(ofSize: 10, weight: .regular)
        buildLabel.textColor = .tertiaryLabelColor
        buildLabel.alignment = .center

        let versionStack = NSStackView(views: [versionLabel, buildLabel])
        versionStack.orientation = .horizontal
        versionStack.alignment = .firstBaseline
        versionStack.spacing = 10

        let brandStack = NSStackView(views: [cardPreview, cardHint, versionStack])
        brandStack.orientation = .vertical
        brandStack.alignment = .centerX
        brandStack.spacing = 9
        brandStack.setCustomSpacing(14, after: cardHint)
        brandStack.translatesAutoresizingMaskIntoConstraints = false
        brandPanel.addSubview(brandStack)

        let nameLabel = NSTextField(labelWithString: "Codex Mixin")
        nameLabel.font = .systemFont(ofSize: 30, weight: .bold)
        nameLabel.textColor = .labelColor

        let sloganLabel = NSTextField(wrappingLabelWithString: L10n.About.slogan)
        sloganLabel.font = .systemFont(ofSize: 17, weight: .medium)
        sloganLabel.textColor = .labelColor

        let detailLabel = NSTextField(wrappingLabelWithString: L10n.About.detail)
        detailLabel.font = .systemFont(ofSize: 13)
        detailLabel.textColor = .secondaryLabelColor
        detailLabel.maximumNumberOfLines = 3
        detailLabel.lineBreakMode = .byWordWrapping

        let repositoryButton = NSButton(
            title: L10n.About.openRepository,
            target: self,
            action: #selector(openRepository(_:))
        )
        repositoryButton.identifier = NSUserInterfaceItemIdentifier("about.repository")
        repositoryButton.bezelStyle = .rounded
        repositoryButton.controlSize = .large
        repositoryButton.image = menuItemImage("arrow.up.right.square")
        repositoryButton.imagePosition = .imageLeading
        repositoryButton.toolTip = info.repositoryURL

        configureCopyButton()
        copyButton.identifier = NSUserInterfaceItemIdentifier("about.copy-version")
        copyButton.target = self
        copyButton.action = #selector(copyVersionInfo(_:))
        copyButton.controlSize = .large

        let buttons = NSStackView(views: [repositoryButton, copyButton])
        buttons.orientation = .horizontal
        buttons.alignment = .centerY
        buttons.distribution = .fillEqually
        buttons.spacing = 10

        let rightStack = NSStackView(views: [
            nameLabel,
            sloganLabel,
            detailLabel,
            buttons,
        ])
        rightStack.orientation = .vertical
        rightStack.alignment = .leading
        rightStack.spacing = 10
        rightStack.setCustomSpacing(4, after: nameLabel)
        rightStack.setCustomSpacing(18, after: detailLabel)
        rightStack.translatesAutoresizingMaskIntoConstraints = false
        buttons.widthAnchor.constraint(equalTo: rightStack.widthAnchor).isActive = true

        contentView.addSubview(brandPanel)
        contentView.addSubview(rightStack)
        NSLayoutConstraint.activate([
            brandPanel.leadingAnchor.constraint(equalTo: contentView.leadingAnchor),
            brandPanel.topAnchor.constraint(equalTo: contentView.topAnchor),
            brandPanel.bottomAnchor.constraint(equalTo: contentView.bottomAnchor),
            brandPanel.widthAnchor.constraint(equalToConstant: 350),

            brandStack.centerXAnchor.constraint(equalTo: brandPanel.centerXAnchor),
            brandStack.centerYAnchor.constraint(equalTo: brandPanel.centerYAnchor, constant: 10),
            cardPreview.widthAnchor.constraint(equalToConstant: 300),
            cardPreview.heightAnchor.constraint(equalToConstant: 188),

            rightStack.leadingAnchor.constraint(equalTo: brandPanel.trailingAnchor, constant: 36),
            rightStack.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -36),
            rightStack.centerYAnchor.constraint(equalTo: contentView.centerYAnchor, constant: 10),
            repositoryButton.heightAnchor.constraint(equalToConstant: 36),
            copyButton.heightAnchor.constraint(equalToConstant: 36),
        ])
    }

    private func configureCopyButton() {
        copyButton.title = L10n.About.copyVersion
        copyButton.bezelStyle = .rounded
        copyButton.image = menuItemImage("doc.on.doc")
        copyButton.imagePosition = .imageLeading
    }
}
