import Cocoa
import SwiftUI

private let menuContentWidth: CGFloat = 336
private let serviceMenuHeight: CGFloat = 56

final class GatewaySwitchControl: NSControl {
    var isOn = false
    var isBusy = false

    func activate(_ newValue: Bool) {
        guard isEnabled, !isBusy else { return }
        isOn = newValue
        sendAction(action, to: target)
    }
}

func gatewayStatusColor(title: String, isRunning: Bool, isBusy: Bool) -> Color {
    if title.contains("失败") { return .red }
    if title.contains("等待配置") || title.contains("降级") || title.contains("无启用") || isBusy {
        return .orange
    }
    return isRunning ? .green : .gray
}

func gatewayStatusDetail(
    title: String,
    endpoint: String?,
    statusDetail: String?,
    isRunning: Bool,
    isBusy: Bool
) -> String {
    if let statusDetail, !statusDetail.isEmpty { return statusDetail }
    if let endpoint { return endpoint }
    if title.contains("失败") { return "请查看运行日志" }
    if title.contains("等待配置") { return "请先设置供应商与 API Key" }
    if isBusy { return "正在切换本地网关" }
    return isRunning ? "正在读取本地接口地址" : "网关当前未运行"
}

final class ServiceMenuModel: ObservableObject {
    @Published var title: String
    @Published var detail: String
    @Published var statusDetail: String?
    @Published var isRunning: Bool
    @Published var isBusy: Bool

    init(
        title: String,
        endpoint: String?,
        statusDetail: String?,
        isRunning: Bool,
        isBusy: Bool
    ) {
        self.title = title
        detail = gatewayStatusDetail(
            title: title,
            endpoint: endpoint,
            statusDetail: statusDetail,
            isRunning: isRunning,
            isBusy: isBusy
        )
        self.statusDetail = statusDetail
        self.isRunning = isRunning
        self.isBusy = isBusy
    }

    func update(
        title: String,
        endpoint: String?,
        statusDetail: String?,
        isRunning: Bool,
        isBusy: Bool
    ) {
        self.title = title
        detail = gatewayStatusDetail(
            title: title,
            endpoint: endpoint,
            statusDetail: statusDetail,
            isRunning: isRunning,
            isBusy: isBusy
        )
        self.statusDetail = statusDetail
        self.isRunning = isRunning
        self.isBusy = isBusy
    }
}

private struct ServiceMenuContent: View {
    @ObservedObject var model: ServiceMenuModel
    let toggle: (Bool) -> Void
    @State private var pulse = false

    var body: some View {
        HStack(spacing: 9) {
            Circle()
                .fill(statusColor)
                .frame(width: 12, height: 12)
                .overlay(Circle().stroke(statusColor.opacity(0.28), lineWidth: 2))
                .shadow(
                    color: statusColor.opacity(shouldPulse ? (pulse ? 0.55 : 0.25) : 0),
                    radius: 3
                )
                .onAppear {
                    guard shouldPulse else { return }
                    withAnimation(.easeInOut(duration: 1.6).repeatForever(autoreverses: true)) {
                        pulse = true
                    }
                }

            VStack(alignment: .leading, spacing: 4) {
                Text(model.title)
                    .font(.callout.weight(.semibold))
                    .lineLimit(1)
                Text(model.detail)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(model.statusDetail ?? "")
            }

            Spacer(minLength: 8)

            ZStack {
                Toggle("本地网关", isOn: Binding(
                    get: { model.isRunning },
                    set: toggle
                ))
                .labelsHidden()
                .toggleStyle(.switch)
                .disabled(model.isBusy)
                .opacity(model.isBusy ? 0.58 : 1)
                if model.isBusy {
                    ProgressView().controlSize(.mini)
                }
            }
        }
        .padding(.horizontal, 14)
        .frame(width: menuContentWidth, height: serviceMenuHeight)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var statusColor: Color {
        gatewayStatusColor(
            title: model.title,
            isRunning: model.isRunning,
            isBusy: model.isBusy
        )
    }

    private var shouldPulse: Bool {
        model.isRunning
            && !model.title.contains("失败")
            && !model.title.contains("等待配置")
            && !model.title.contains("降级")
            && !model.title.contains("无启用")
            && !model.isBusy
            && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
    }
}

final class ServiceMenuHostingView: NSView {
    let model: ServiceMenuModel
    let bridgeControl: GatewaySwitchControl
    private let hostingView: NSHostingView<ServiceMenuContent>

    init(
        title: String,
        endpoint: String?,
        statusDetail: String?,
        isRunning: Bool,
        isBusy: Bool,
        target: AnyObject?,
        action: Selector
    ) {
        model = ServiceMenuModel(
            title: title,
            endpoint: endpoint,
            statusDetail: statusDetail,
            isRunning: isRunning,
            isBusy: isBusy
        )
        bridgeControl = GatewaySwitchControl()
        bridgeControl.isOn = isRunning
        bridgeControl.isBusy = isBusy
        bridgeControl.isEnabled = !isBusy
        bridgeControl.target = target
        bridgeControl.action = action
        hostingView = NSHostingView(rootView: ServiceMenuContent(
            model: model,
            toggle: bridgeControl.activate
        ))
        super.init(frame: NSRect(x: 0, y: 0, width: menuContentWidth, height: serviceMenuHeight))
        hostingView.frame = bounds
        hostingView.autoresizingMask = [.width, .height]
        addSubview(hostingView)
        bridgeControl.frame = .zero
        bridgeControl.isHidden = true
        addSubview(bridgeControl)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }
}

func updateServiceMenuView(
    _ view: NSView,
    title: String,
    endpoint: String?,
    statusDetail: String?,
    isRunning: Bool,
    isBusy: Bool
) -> Bool {
    guard let serviceView = view as? ServiceMenuHostingView else { return false }
    serviceView.model.update(
        title: title,
        endpoint: endpoint,
        statusDetail: statusDetail,
        isRunning: isRunning,
        isBusy: isBusy
    )
    serviceView.bridgeControl.isOn = isRunning
    serviceView.bridgeControl.isBusy = isBusy
    serviceView.bridgeControl.isEnabled = !isBusy
    return true
}

func serviceMenuView(
    title: String,
    endpoint: String?,
    statusDetail: String?,
    isRunning: Bool,
    isBusy: Bool,
    target: AnyObject?,
    action: Selector
) -> NSView {
    ServiceMenuHostingView(
        title: title,
        endpoint: endpoint,
        statusDetail: statusDetail,
        isRunning: isRunning,
        isBusy: isBusy,
        target: target,
        action: action
    )
}
