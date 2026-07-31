import AppKit
import CryptoKit
import SwiftUI
import UniformTypeIdentifiers

struct CardIdentityV1: Codable, Equatable {
    let installationID: UUID
    let firstRecordedAt: Date
    let seedVersion: UInt8
}

final class CardIdentityStore {
    static let standard = CardIdentityStore()

    private let defaults: UserDefaults
    private let key: String

    init(
        defaults: UserDefaults = .standard,
        key: String = "codexMixin.cardIdentity.v1"
    ) {
        self.defaults = defaults
        self.key = key
    }

    @discardableResult
    func current(
        now: Date = Date(),
        makeUUID: () -> UUID = UUID.init
    ) -> CardIdentityV1 {
        if
            let data = defaults.data(forKey: key),
            let identity = try? JSONDecoder().decode(CardIdentityV1.self, from: data),
            identity.seedVersion == 1
        {
            return identity
        }

        let identity = CardIdentityV1(
            installationID: makeUUID(),
            firstRecordedAt: now,
            seedVersion: 1
        )
        if let data = try? JSONEncoder().encode(identity) {
            defaults.set(data, forKey: key)
        }
        return identity
    }

    func reset() {
        defaults.removeObject(forKey: key)
    }
}

struct CardWallpaper: Codable, Equatable {
    let fileName: String
    let title: String
    let credit: String
    let sourceURL: String
    let width: Int
    let height: Int
    let sha256: String
}

private struct CardWallpaperManifest: Codable {
    let schemaVersion: Int
    let issue: String
    let sourcePage: String
    let images: [CardWallpaper]
}

enum CardWallpaperCatalog {
    private static let manifest: CardWallpaperManifest? = {
        guard
            let url = directoryURL?.appendingPathComponent("manifest.json"),
            let data = try? Data(contentsOf: url),
            let manifest = try? JSONDecoder().decode(CardWallpaperManifest.self, from: data),
            manifest.schemaVersion == 1
        else {
            return nil
        }
        return manifest
    }()

    static var issue: String {
        manifest?.issue ?? "SPACE ARCHIVE"
    }

    static var sourcePage: String? {
        manifest?.sourcePage
    }

    static var wallpapers: [CardWallpaper] {
        manifest?.images ?? []
    }

    static func image(for wallpaper: CardWallpaper) -> NSImage? {
        guard let url = directoryURL?.appendingPathComponent(wallpaper.fileName) else {
            return nil
        }
        return NSImage(contentsOf: url)
    }

    private static var directoryURL: URL? {
        if let directory = ProcessInfo.processInfo.environment["CODEX_MIXIN_WALLPAPER_ASSET_DIR"] {
            return URL(fileURLWithPath: directory, isDirectory: true)
        }
        return Bundle.main.resourceURL?.appendingPathComponent(
            "Wallpapers",
            isDirectory: true
        )
    }
}

struct InstallCardDesign: Equatable {
    let seed: UInt64
    let wallpaperIndex: Int
    let wallpaper: CardWallpaper?
    let identityCode: String

    init(
        identity: CardIdentityV1,
        wallpapers: [CardWallpaper] = CardWallpaperCatalog.wallpapers
    ) {
        let digest = cardIdentityDigest(identity)
        seed = digest.prefix(8).reduce(UInt64.zero) { ($0 << 8) | UInt64($1) }
        wallpaperIndex = wallpapers.isEmpty ? 0 : Int(seed % UInt64(wallpapers.count))
        wallpaper = wallpapers.isEmpty ? nil : wallpapers[wallpaperIndex]
        identityCode = digest.prefix(4).map { String(format: "%02X", $0) }.joined()
    }
}

func cardIdentityDigest(_ identity: CardIdentityV1) -> [UInt8] {
    let recordedDay = Int64(floor(identity.firstRecordedAt.timeIntervalSince1970 / 86_400))
    let payload = [
        "codex-mixin-card",
        identity.installationID.uuidString.lowercased(),
        String(recordedDay),
        String(identity.seedVersion),
    ].joined(separator: "|")
    return Array(SHA256.hash(data: Data(payload.utf8)))
}

func cardDayCount(
    identity: CardIdentityV1,
    now: Date = Date(),
    calendar: Calendar = .current
) -> Int {
    let start = calendar.startOfDay(for: identity.firstRecordedAt)
    let end = calendar.startOfDay(for: max(now, identity.firstRecordedAt))
    return max(1, (calendar.dateComponents([.day], from: start, to: end).day ?? 0) + 1)
}

func cardRecordedMonth(identity: CardIdentityV1, locale: Locale = .current) -> String {
    let formatter = DateFormatter()
    formatter.locale = locale
    formatter.setLocalizedDateFormatFromTemplate("yyyyMMM")
    return formatter.string(from: identity.firstRecordedAt)
}

private func cardWallpaperIssueLabel(_ issue: String) -> String {
    let parts = issue.split(separator: "-")
    guard
        parts.count == 2,
        let year = Int(parts[0]),
        let month = Int(parts[1]),
        (1...12).contains(month)
    else {
        return issue.uppercased()
    }
    let formatter = DateFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    let symbols = formatter.shortMonthSymbols ?? []
    let monthName = month <= symbols.count ? symbols[month - 1] : String(month)
    return "\(monthName.uppercased()) \(year)"
}

struct InstallCardSurface: View {
    let identity: CardIdentityV1
    let elapsed: TimeInterval
    let pointer: CGPoint
    let drag: CGSize
    let revealed: Bool
    let now: Date

    private var design: InstallCardDesign {
        InstallCardDesign(identity: identity)
    }

    var body: some View {
        GeometryReader { geometry in
            let size = geometry.size
            ZStack {
                wallpaper(size: size)
                readabilityOverlay
                    .frame(width: size.width, height: size.height)
                cardCopy(size: size)
                    .frame(width: size.width, height: size.height)
            }
            .frame(width: size.width, height: size.height)
            .clipped()
        }
        .aspectRatio(1.6, contentMode: .fit)
        .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 20, style: .continuous)
                .stroke(Color.white.opacity(0.22), lineWidth: 1)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            appText(
                "Codex Mixin 纪念卡，Mixin 认识你的第 \(cardDayCount(identity: identity, now: now)) 天",
                "Codex Mixin 紀念卡，Mixin 認識你的第 \(cardDayCount(identity: identity, now: now)) 天",
                "Codex Mixin card, day \(cardDayCount(identity: identity, now: now)) with Mixin"
            )
        )
    }

    @ViewBuilder
    private func wallpaper(size: CGSize) -> some View {
        if
            let selected = design.wallpaper,
            let image = CardWallpaperCatalog.image(for: selected)
        {
            let parallaxX = (pointer.x - 0.5) * size.width * 0.024 + drag.width * 0.035
            let parallaxY = (pointer.y - 0.5) * size.height * 0.03 + drag.height * 0.035
            Image(nsImage: image)
                .resizable()
                .scaledToFill()
                .frame(width: size.width * 1.07, height: size.height * 1.07)
                .offset(x: parallaxX, y: parallaxY)
                .scaleEffect(revealed ? 1.025 : 1)
                .saturation(revealed ? 1.08 : 1)
                .animation(.spring(response: 0.5, dampingFraction: 0.82), value: revealed)
        } else {
            LinearGradient(
                colors: [Color(red: 0.02, green: 0.05, blue: 0.1), .black],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
        }
    }

    private var readabilityOverlay: some View {
        ZStack {
            LinearGradient(
                colors: [
                    .black.opacity(0.82),
                    .black.opacity(0.47),
                    .black.opacity(0.06),
                    .clear,
                ],
                startPoint: .leading,
                endPoint: .trailing
            )
            LinearGradient(
                colors: [.clear, .black.opacity(0.52)],
                startPoint: .center,
                endPoint: .bottom
            )
        }
    }

    @ViewBuilder
    private func cardCopy(size: CGSize) -> some View {
        let dayCount = cardDayCount(identity: identity, now: now)
        let edge = max(22, size.width * 0.038)
        let recordedMonth = cardRecordedMonth(
            identity: identity,
            locale: Locale(identifier: "en_US")
        ).uppercased()

        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                HStack(spacing: 8) {
                    Circle()
                        .fill(Color.white)
                        .frame(width: max(4, size.width * 0.006))
                    Text("CODEX MIXIN")
                        .font(.system(
                            size: max(11, size.width * 0.016),
                            weight: .semibold,
                            design: .monospaced
                        ))
                        .tracking(1.6)
                }
                Spacer()
                Text(cardWallpaperIssueLabel(CardWallpaperCatalog.issue))
                    .font(.system(
                        size: max(9, size.width * 0.012),
                        weight: .medium,
                        design: .monospaced
                    ))
                    .tracking(1.2)
                    .opacity(0.74)
            }

            Spacer()

            HStack(alignment: .bottom, spacing: 22) {
                VStack(alignment: .leading, spacing: 0) {
                    Text(String(format: "%03d", dayCount))
                        .font(.system(
                            size: max(44, size.width * 0.09),
                            weight: .medium,
                            design: .rounded
                        ))
                        .tracking(-2)
                        .lineLimit(1)
                        .minimumScaleFactor(0.7)

                    Text(revealed
                        ? appText("我们仍在轨道上", "我們仍在軌道上", "STILL IN ORBIT")
                        : appText("与 MIXIN 相伴的日子", "與 MIXIN 相伴的日子", "DAYS WITH MIXIN")
                    )
                    .font(.system(
                        size: max(11, size.width * 0.017),
                        weight: .semibold,
                        design: .monospaced
                    ))
                    .tracking(1.2)
                    .opacity(0.92)

                    Rectangle()
                        .fill(Color.white.opacity(0.5))
                        .frame(width: max(70, size.width * 0.12), height: 1)
                        .padding(.vertical, max(9, size.height * 0.018))

                    Text("SINCE \(recordedMonth)  ·  \(design.identityCode)")
                    .font(.system(
                        size: max(8, size.width * 0.011),
                        weight: .medium,
                        design: .monospaced
                    ))
                    .tracking(0.9)
                    .opacity(0.68)
                }

                Spacer(minLength: 16)

                if let selected = design.wallpaper {
                    VStack(alignment: .trailing, spacing: 4) {
                        Text(selected.title.uppercased())
                            .font(.system(
                                size: max(9, size.width * 0.013),
                                weight: .semibold,
                                design: .monospaced
                            ))
                            .tracking(0.9)
                            .multilineTextAlignment(.trailing)
                        Text("IMAGE  \(selected.credit.uppercased())")
                            .font(.system(
                                size: max(8, size.width * 0.01),
                                weight: .regular,
                                design: .monospaced
                            ))
                            .tracking(0.8)
                            .opacity(0.64)
                    }
                    .frame(maxWidth: size.width * 0.27, alignment: .trailing)
                }
            }
        }
        .foregroundStyle(.white)
        .padding(edge)
        .shadow(color: .black.opacity(0.35), radius: 10, y: 3)
    }
}

struct InstallCardExperienceView: View {
    let identity: CardIdentityV1
    let onSave: (_ revealed: Bool) -> Void
    let onShare: (_ revealed: Bool) -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var pointer = CGPoint(x: 0.5, y: 0.5)
    @State private var drag = CGSize.zero
    @State private var revealed = false
    @State private var hovering = false

    var body: some View {
        VStack(spacing: 18) {
            GeometryReader { geometry in
                InstallCardSurface(
                    identity: identity,
                    elapsed: 0,
                    pointer: pointer,
                    drag: drag,
                    revealed: revealed,
                    now: Date()
                )
                .scaleEffect(hovering && !reduceMotion ? 1.008 : 1)
                .shadow(
                    color: .black.opacity(hovering ? 0.34 : 0.24),
                    radius: hovering ? 28 : 18,
                    y: hovering ? 16 : 10
                )
                .contentShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
                .onContinuousHover { phase in
                    switch phase {
                    case .active(let location):
                        hovering = true
                        guard !reduceMotion else { return }
                        pointer = CGPoint(
                            x: min(1, max(0, location.x / max(1, geometry.size.width))),
                            y: min(1, max(0, location.y / max(1, geometry.size.height)))
                        )
                    case .ended:
                        withAnimation(.spring(response: 0.42, dampingFraction: 0.82)) {
                            hovering = false
                            pointer = CGPoint(x: 0.5, y: 0.5)
                        }
                    }
                }
                .gesture(
                    DragGesture(minimumDistance: 2)
                        .onChanged { value in
                            guard !reduceMotion else { return }
                            drag = value.translation
                        }
                        .onEnded { _ in
                            withAnimation(.spring(response: 0.5, dampingFraction: 0.72)) {
                                drag = .zero
                            }
                        }
                )
                .onTapGesture {
                    withAnimation(.spring(response: 0.46, dampingFraction: 0.78)) {
                        revealed.toggle()
                    }
                }
                .help(appText(
                    "移动、拖动或点击卡片",
                    "移動、拖動或點擊卡片",
                    "Move, drag, or click the card"
                ))
            }
            .aspectRatio(1.6, contentMode: .fit)

            HStack(spacing: 10) {
                Label(
                    appText("移动、拖动、点击都有反应", "移動、拖動、點擊都有反應", "Move, drag, and click"),
                    systemImage: "cursorarrow.motionlines"
                )
                .font(.system(size: 12))
                .foregroundStyle(.secondary)

                Spacer()

                Button {
                    onSave(revealed)
                } label: {
                    Label(appText("保存 PNG", "儲存 PNG", "Save PNG"), systemImage: "square.and.arrow.down")
                }

                Button {
                    onShare(revealed)
                } label: {
                    Label(appText("分享", "分享", "Share"), systemImage: "square.and.arrow.up")
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(28)
        .frame(minWidth: 720, minHeight: 520)
    }
}

struct InstallCardThumbnailView: View {
    let identity: CardIdentityV1
    let onOpen: () -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var hovering = false

    var body: some View {
        Button(action: onOpen) {
            InstallCardSurface(
                identity: identity,
                elapsed: 0,
                pointer: hovering && !reduceMotion
                    ? CGPoint(x: 0.58, y: 0.42)
                    : CGPoint(x: 0.5, y: 0.5),
                drag: .zero,
                revealed: false,
                now: Date()
            )
        }
        .buttonStyle(.plain)
        .scaleEffect(hovering && !reduceMotion ? 1.018 : 1)
        .shadow(
            color: .black.opacity(hovering ? 0.32 : 0.2),
            radius: hovering ? 18 : 10,
            y: hovering ? 10 : 6
        )
        .animation(.spring(response: 0.34, dampingFraction: 0.82), value: hovering)
        .onHover { hovering = $0 }
        .help(appText("点击放大 Mixin 卡片", "點擊放大 Mixin 卡片", "Click to enlarge the Mixin card"))
        .accessibilityLabel(appText("打开我的 Mixin 卡片", "打開我的 Mixin 卡片", "Open My Mixin Card"))
    }
}

@MainActor
func renderInstallCardPNG(
    identity: CardIdentityV1,
    revealed: Bool,
    now: Date = Date(),
    size: CGSize = CGSize(width: 1_200, height: 750)
) -> Data? {
    let surface = InstallCardSurface(
        identity: identity,
        elapsed: 0,
        pointer: CGPoint(x: 0.5, y: 0.5),
        drag: .zero,
        revealed: revealed,
        now: now
    )
    .frame(width: size.width, height: size.height)

    let renderer = ImageRenderer(content: surface)
    renderer.proposedSize = ProposedViewSize(size)
    renderer.scale = 1
    guard
        let image = renderer.nsImage,
        let tiff = image.tiffRepresentation,
        let bitmap = NSBitmapImageRep(data: tiff)
    else {
        return nil
    }
    return bitmap.representation(using: .png, properties: [:])
}

final class InstallCardWindowController: NSWindowController, NSWindowDelegate {
    private let identity: CardIdentityV1
    private var sharingPicker: NSSharingServicePicker?

    init(identityStore: CardIdentityStore = .standard) {
        identity = identityStore.current()
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 820, height: 610),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = appText("我的 Mixin 卡片", "我的 Mixin 卡片", "My Mixin Card")
        window.minSize = NSSize(width: 720, height: 560)
        window.isReleasedWhenClosed = false
        window.center()

        super.init(window: window)
        window.delegate = self
        installContent(in: window)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() {
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func installContent(in window: NSWindow) {
        let rootView = InstallCardExperienceView(
            identity: identity,
            onSave: { [weak self] revealed in
                self?.savePNG(revealed: revealed)
            },
            onShare: { [weak self] revealed in
                self?.sharePNG(revealed: revealed)
            }
        )
        window.contentViewController = NSHostingController(rootView: rootView)
    }

    private func savePNG(revealed: Bool) {
        guard
            let window,
            let data = renderInstallCardPNG(identity: identity, revealed: revealed)
        else {
            showAlert(
                title: appText("无法生成卡片", "無法生成卡片", "Could Not Render Card"),
                message: appText("请重试一次。", "請重試一次。", "Please try again.")
            )
            return
        }

        let panel = NSSavePanel()
        panel.allowedContentTypes = [.png]
        panel.canCreateDirectories = true
        panel.nameFieldStringValue = "codex-mixin-card.png"
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let url = panel.url else { return }
            do {
                try data.write(to: url, options: .atomic)
            } catch {
                showAlert(
                    title: appText("保存失败", "儲存失敗", "Save Failed"),
                    message: String(describing: error)
                )
            }
        }
    }

    private func sharePNG(revealed: Bool) {
        guard
            let window,
            let anchor = window.contentView,
            let data = renderInstallCardPNG(identity: identity, revealed: revealed)
        else {
            showAlert(
                title: appText("无法生成卡片", "無法生成卡片", "Could Not Render Card"),
                message: appText("请重试一次。", "請重試一次。", "Please try again.")
            )
            return
        }

        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("codex-mixin-card-\(UUID().uuidString).png")
        do {
            try data.write(to: url, options: .atomic)
            let picker = NSSharingServicePicker(items: [url])
            sharingPicker = picker
            picker.show(
                relativeTo: NSRect(x: anchor.bounds.midX, y: 0, width: 1, height: 1),
                of: anchor,
                preferredEdge: .minY
            )
        } catch {
            showAlert(
                title: appText("分享失败", "分享失敗", "Share Failed"),
                message: String(describing: error)
            )
        }
    }
}
