import Cocoa
import CryptoKit
import SwiftUI

func showAlert(title: String, message: String) {
    preconditionFailure("Unexpected alert: \(title): \(message)")
}

@main
struct InstallCardTests {
    @MainActor
    static func main() {
        _ = NSApplication.shared

        let suiteName = "local.codex-mixin.install-card-tests.\(UUID().uuidString)"
        guard let defaults = UserDefaults(suiteName: suiteName) else {
            preconditionFailure("Test defaults suite must be available")
        }
        defer {
            defaults.removePersistentDomain(forName: suiteName)
        }

        let store = CardIdentityStore(
            defaults: defaults,
            earliestHistoryDate: { nil }
        )
        let expectedUUID = UUID(uuidString: "59B26835-9033-4B89-B8CC-57ACB42C8C9B")!
        let firstDate = Date(timeIntervalSince1970: 1_722_470_400)
        let first = store.current(now: firstDate, makeUUID: { expectedUUID })
        let second = store.current(
            now: firstDate.addingTimeInterval(86_400),
            makeUUID: { preconditionFailure("Stored identity must be reused") }
        )
        precondition(first == second)
        precondition(first.installationID == expectedUUID)
        precondition(first.seedVersion == 2)

        let historicalDate = firstDate.addingTimeInterval(-22 * 86_400)
        let migratedStore = CardIdentityStore(
            defaults: defaults,
            earliestHistoryDate: { historicalDate }
        )
        let migrated = migratedStore.current(
            now: firstDate.addingTimeInterval(86_400),
            makeUUID: { preconditionFailure("Migration must preserve the stored UUID") }
        )
        precondition(migrated.installationID == expectedUUID)
        precondition(migrated.firstRecordedAt == historicalDate)
        precondition(migrated.seedVersion == 2)
        precondition(cardDayCount(identity: migrated, now: firstDate) == 23)

        let firstDesign = InstallCardDesign(identity: migrated)
        let repeatedDesign = InstallCardDesign(identity: migratedStore.current(now: firstDate))
        precondition(firstDesign == repeatedDesign)
        precondition(CardWallpaperCatalog.issue == "2026-07")
        precondition(CardWallpaperCatalog.wallpapers.count == 3)
        precondition(firstDesign.wallpaper != nil)
        precondition(CardWallpaperCatalog.wallpapers.allSatisfy {
            CardWallpaperCatalog.image(for: $0) != nil
        })
        let cycledWallpaperIndices = (0..<CardWallpaperCatalog.wallpapers.count).map {
            InstallCardDesign(identity: migrated, wallpaperOffset: $0).wallpaperIndex
        }
        precondition(Set(cycledWallpaperIndices).count == CardWallpaperCatalog.wallpapers.count)
        precondition(
            InstallCardDesign(
                identity: migrated,
                wallpaperOffset: CardWallpaperCatalog.wallpapers.count
            ).wallpaperIndex == firstDesign.wallpaperIndex
        )

        let selectionDefaultsName = "\(suiteName).wallpaper-selection"
        guard let selectionDefaults = UserDefaults(suiteName: selectionDefaultsName) else {
            preconditionFailure("Wallpaper selection defaults suite must be available")
        }
        defer {
            selectionDefaults.removePersistentDomain(forName: selectionDefaultsName)
        }
        let selectionStore = CardWallpaperSelectionStore(defaults: selectionDefaults)
        let selectedOffsets = [
            selectionStore.nextOffset(count: 3, randomIndex: { _ in 1 }),
            selectionStore.nextOffset(count: 3, randomIndex: { _ in 1 }),
            selectionStore.nextOffset(count: 3, randomIndex: { _ in 0 }),
        ]
        precondition(selectedOffsets == [1, 2, 0])
        precondition(
            zip(selectedOffsets, selectedOffsets.dropFirst()).allSatisfy {
                $0.0 != $0.1
            }
        )

        let laterDate = firstDate.addingTimeInterval(4 * 86_400)
        precondition(cardDayCount(identity: first, now: laterDate) == 5)

        let otherIdentity = CardIdentityV1(
            installationID: UUID(uuidString: "1900DD13-DC6A-4511-A2FB-2D7FCD9A76A2")!,
            firstRecordedAt: firstDate,
            seedVersion: 2
        )
        precondition(InstallCardDesign(identity: otherIdentity) != firstDesign)

        guard let png = renderInstallCardPNG(
            identity: migrated,
            revealed: false,
            now: laterDate
        ) else {
            preconditionFailure("Card renderer must return PNG data")
        }
        precondition(png.count > 100_000)
        precondition(Array(png.prefix(8)) == [137, 80, 78, 71, 13, 10, 26, 10])
        guard let nextWallpaperPNG = renderInstallCardPNG(
            identity: migrated,
            revealed: false,
            wallpaperOffset: 1,
            now: laterDate
        ) else {
            preconditionFailure("Card renderer must export the active wallpaper")
        }
        precondition(nextWallpaperPNG != png)

        if let snapshotPath = ProcessInfo.processInfo.environment["INSTALL_CARD_SNAPSHOT"] {
            do {
                try png.write(to: URL(fileURLWithPath: snapshotPath), options: .atomic)
            } catch {
                preconditionFailure("Card snapshot could not be written: \(error)")
            }
        }

        print("Install card identity, design, and PNG rendering: passed")
    }
}
