import Cocoa
import SwiftUI

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

@MainActor
final class AboutModel: ObservableObject {
    let info: AppAboutInfo
    let cardIdentity: CardIdentityV1
    let wallpaperOffset: Int
    @Published var copied = false

    private let openURL: (URL) -> Void
    private let copyText: (String) -> Void
    private let showCard: (Int) -> Void

    init(
        info: AppAboutInfo,
        cardIdentity: CardIdentityV1,
        wallpaperOffset: Int,
        openURL: @escaping (URL) -> Void,
        copyText: @escaping (String) -> Void,
        showCard: @escaping (Int) -> Void
    ) {
        self.info = info
        self.cardIdentity = cardIdentity
        self.wallpaperOffset = wallpaperOffset
        self.openURL = openURL
        self.copyText = copyText
        self.showCard = showCard
    }

    func openRepository() {
        guard let url = URL(string: info.repositoryURL) else { return }
        openURL(url)
    }

    func copyVersionInfo() {
        copyText(info.versionSummary)
        copied = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
            self?.copied = false
        }
    }

    func openCard() {
        showCard(wallpaperOffset)
    }
}

private struct AboutView: View {
    @ObservedObject var model: AboutModel

    var body: some View {
        HStack(spacing: 0) {
            VStack(spacing: 12) {
                InstallCardThumbnailView(
                    identity: model.cardIdentity,
                    wallpaperOffset: model.wallpaperOffset,
                    onOpen: model.openCard
                )
                .frame(width: 300, height: 188)
                .accessibilityIdentifier("about.card-preview")

                Text(L10n.About.cardHint)
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)

                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Text(L10n.About.version(model.info.version))
                        .font(.caption.monospaced().weight(.medium))
                    Text("Build \(model.info.build)")
                        .font(.caption2.monospaced())
                        .foregroundStyle(.tertiary)
                }
                .foregroundStyle(.secondary)
            }
            .frame(width: 350)
            .frame(maxHeight: .infinity)
            .background(.ultraThinMaterial)

            VStack(alignment: .leading, spacing: 10) {
                Text("Codex Mixin")
                    .font(.system(size: 30, weight: .bold, design: .rounded))
                Text(L10n.About.slogan)
                    .font(.title3.weight(.medium))
                Text(L10n.About.detail)
                    .font(.body)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                HStack(spacing: 10) {
                    Button(action: model.openRepository) {
                        Label(L10n.About.openRepository, systemImage: "arrow.up.right.square")
                            .frame(maxWidth: .infinity)
                    }
                    .help(model.info.repositoryURL)
                    .accessibilityIdentifier("about.repository")

                    Button(action: model.copyVersionInfo) {
                        Label(
                            model.copied ? L10n.About.copied : L10n.About.copyVersion,
                            systemImage: model.copied ? "checkmark" : "doc.on.doc"
                        )
                        .frame(maxWidth: .infinity)
                    }
                    .accessibilityIdentifier("about.copy-version")
                }
                .controlSize(.large)
                .padding(.top, 8)
            }
            .padding(36)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        }
        .frame(width: 820, height: 460)
        .background(Color(nsColor: .windowBackgroundColor))
    }
}

final class AboutWindowController: NSWindowController, NSWindowDelegate {
    typealias OpenURLHandler = (URL) -> Void
    typealias CopyTextHandler = (String) -> Void
    typealias ShowCardHandler = (_ wallpaperOffset: Int) -> Void

    let model: AboutModel

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
        model = AboutModel(
            info: info,
            cardIdentity: cardIdentity,
            wallpaperOffset: wallpaperOffset,
            openURL: openURL,
            copyText: copyText,
            showCard: showCard
        )
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
        window.center()
        super.init(window: window)
        window.delegate = self
        window.contentViewController = NSHostingController(rootView: AboutView(model: model))
        window.setFrame(
            NSRect(origin: window.frame.origin, size: NSSize(width: 820, height: 460)),
            display: false
        )
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() {
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
}
