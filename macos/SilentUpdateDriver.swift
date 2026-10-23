import Cocoa
import Sparkle

/// Sparkle owns signature verification, staging, replacement and relaunch.
/// Background cycles never enter the standard driver's alert/DMG paths.
@MainActor
final class SilentUpdateDriver: SPUStandardUserDriver {
    var silent = false
    var pendingInstall: ((SPUUserUpdateChoice) -> Void)?

    func cancelPendingInstall() {
        let reply = pendingInstall
        pendingInstall = nil
        reply?(.skip)
    }

    override func show(_ request: SPUUpdatePermissionRequest, reply: @escaping (SUUpdatePermissionResponse) -> Void) {
        reply(SUUpdatePermissionResponse(automaticUpdateChecks: false, sendSystemProfile: false))
    }

    override func showUserInitiatedUpdateCheck(cancellation: @escaping () -> Void) {
        if !silent { super.showUserInitiatedUpdateCheck(cancellation: cancellation) }
    }

    override func showUpdateFound(with appcastItem: SUAppcastItem, state: SPUUserUpdateState, reply: @escaping (SPUUserUpdateChoice) -> Void) {
        guard silent else {
            super.showUpdateFound(with: appcastItem, state: state, reply: reply)
            return
        }
        guard !appcastItem.isInformationOnlyUpdate, !appcastItem.isMajorUpgrade,
              appcastItem.installationType == "application"
        else { reply(.dismiss); return }
        reply(.install)
    }

    override func showUpdateReleaseNotes(with downloadData: SPUDownloadData) {
        if !silent { super.showUpdateReleaseNotes(with: downloadData) }
    }

    override func showUpdateReleaseNotesFailedToDownloadWithError(_ error: Error) {
        if !silent { super.showUpdateReleaseNotesFailedToDownloadWithError(error) }
    }

    override func showUpdateNotFoundWithError(_ error: Error, acknowledgement: @escaping () -> Void) {
        if silent { acknowledgement() } else { super.showUpdateNotFoundWithError(error, acknowledgement: acknowledgement) }
    }

    override func showUpdaterError(_ error: Error, acknowledgement: @escaping () -> Void) {
        if silent { acknowledgement() } else { super.showUpdaterError(error, acknowledgement: acknowledgement) }
    }

    override func showDownloadInitiated(cancellation: @escaping () -> Void) {
        if !silent { super.showDownloadInitiated(cancellation: cancellation) }
    }

    override func showDownloadDidReceiveExpectedContentLength(_ expectedContentLength: UInt64) {
        if !silent { super.showDownloadDidReceiveExpectedContentLength(expectedContentLength) }
    }

    override func showDownloadDidReceiveData(ofLength length: UInt64) {
        if !silent { super.showDownloadDidReceiveData(ofLength: length) }
    }

    override func showDownloadDidStartExtractingUpdate() {
        if !silent { super.showDownloadDidStartExtractingUpdate() }
    }

    override func showExtractionReceivedProgress(_ progress: Double) {
        if !silent { super.showExtractionReceivedProgress(progress) }
    }

    override func showReady(toInstallAndRelaunch reply: @escaping (SPUUserUpdateChoice) -> Void) {
        if silent { pendingInstall = reply } else { super.showReady(toInstallAndRelaunch: reply) }
    }

    override func showInstallingUpdate(withApplicationTerminated applicationTerminated: Bool, retryTerminatingApplication: @escaping () -> Void) {
        if !silent { super.showInstallingUpdate(withApplicationTerminated: applicationTerminated, retryTerminatingApplication: retryTerminatingApplication) }
    }

    override func showUpdateInstalledAndRelaunched(_ relaunched: Bool, acknowledgement: @escaping () -> Void) {
        if silent { acknowledgement() } else { super.showUpdateInstalledAndRelaunched(relaunched, acknowledgement: acknowledgement) }
    }

    override func dismissUpdateInstallation() {
        if !silent { super.dismissUpdateInstallation() }
    }

    override func showUpdateInFocus() {
        if !silent { super.showUpdateInFocus() }
    }
}
