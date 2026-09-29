import Foundation

func menuLaunchAgentPlist(
    label: String,
    executablePath: String,
    logPath: String
) -> String {
    """
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
      <key>Label</key>
      <string>\(launchAgentXMLescape(label))</string>
      <key>ProgramArguments</key>
      <array>
        <string>\(launchAgentXMLescape(executablePath))</string>
      </array>
      <key>RunAtLoad</key>
      <true/>
      <key>KeepAlive</key>
      <dict>
        <key>SuccessfulExit</key>
        <false/>
      </dict>
      <key>ThrottleInterval</key>
      <integer>10</integer>
      <key>ProcessType</key>
      <string>Interactive</string>
      <key>StandardOutPath</key>
      <string>\(launchAgentXMLescape(logPath))</string>
      <key>StandardErrorPath</key>
      <string>\(launchAgentXMLescape(logPath))</string>
    </dict>
    </plist>
    """
}

private func launchAgentXMLescape(_ value: String) -> String {
    value
        .replacingOccurrences(of: "&", with: "&amp;")
        .replacingOccurrences(of: "<", with: "&lt;")
        .replacingOccurrences(of: ">", with: "&gt;")
        .replacingOccurrences(of: "\"", with: "&quot;")
        .replacingOccurrences(of: "'", with: "&apos;")
}
