import Foundation
import Darwin

struct ProcessOutputResult {
    let terminationStatus: Int32
    let data: Data
}

private final class ProcessTimeout {
    private let terminateWork: DispatchWorkItem
    private let killWork: DispatchWorkItem

    init(process: Process, timeout: TimeInterval, killGrace: TimeInterval) {
        let queue = DispatchQueue(label: "local.codex-mixin.process-timeout")
        let terminateWork = DispatchWorkItem { [process] in
            if process.isRunning {
                process.terminate()
            }
        }
        let killWork = DispatchWorkItem { [process] in
            if process.isRunning {
                kill(process.processIdentifier, SIGKILL)
            }
        }
        self.terminateWork = terminateWork
        self.killWork = killWork
        guard timeout > 0 else { return }
        queue.asyncAfter(deadline: .now() + timeout, execute: terminateWork)
        queue.asyncAfter(deadline: .now() + timeout + killGrace, execute: killWork)
    }

    func cancel() {
        terminateWork.cancel()
        killWork.cancel()
    }
}

func runProcessCollectingMergedOutput(
    _ process: Process,
    outputPipe: Pipe,
    timeout: TimeInterval = 0,
    killGrace: TimeInterval = 10
) throws -> ProcessOutputResult {
    try process.run()
    let timeout = ProcessTimeout(
        process: process,
        timeout: timeout,
        killGrace: killGrace
    )
    defer { timeout.cancel() }

    // Drain the pipe before waiting so a verbose child cannot block on a full buffer.
    let data = outputPipe.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()

    return ProcessOutputResult(
        terminationStatus: process.terminationStatus,
        data: data
    )
}

final class StreamingProcessOutputCollector {
    private let queue = DispatchQueue(label: "local.codex-mixin.streaming-process-output")
    private let progressQueue: DispatchQueue
    private let onProgress: (String) -> Void
    private let progressPrefix: String?
    private var combinedData = Data()
    private var pendingBuffer = ""

    init(
        progressPrefix: String? = nil,
        progressQueue: DispatchQueue = .main,
        onProgress: @escaping (String) -> Void
    ) {
        self.progressPrefix = progressPrefix
        self.progressQueue = progressQueue
        self.onProgress = onProgress
    }

    func consume(_ data: Data) {
        guard !data.isEmpty else { return }
        queue.async { [self] in
            consumeOnQueue(data)
        }
    }

    func finish(remainingData: [Data]) -> Data {
        queue.sync {
            for data in remainingData where !data.isEmpty {
                consumeOnQueue(data)
            }
            emitProgressOnQueue(pendingBuffer)
            pendingBuffer = ""
            return combinedData
        }
    }

    private func consumeOnQueue(_ data: Data) {
        combinedData.append(data)
        pendingBuffer += String(decoding: data, as: UTF8.self)
        let parts = pendingBuffer.components(separatedBy: .newlines)
        pendingBuffer = parts.last ?? ""
        for line in parts.dropLast() {
            emitProgressOnQueue(line)
        }
    }

    private func emitProgressOnQueue(_ rawLine: String) {
        let line = rawLine.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !line.isEmpty else { return }
        let progress: String
        if let progressPrefix {
            guard line.hasPrefix(progressPrefix) else { return }
            progress = String(line.dropFirst(progressPrefix.count))
        } else {
            progress = line
        }
        progressQueue.async { [onProgress] in
            onProgress(progress)
        }
    }
}

func runProcessCollectingStreamingOutput(
    _ process: Process,
    outputPipe: Pipe,
    errorPipe: Pipe,
    collector: StreamingProcessOutputCollector
) throws -> ProcessOutputResult {
    try process.run()

    let drainGroup = DispatchGroup()
    for handle in [
        outputPipe.fileHandleForReading,
        errorPipe.fileHandleForReading,
    ] {
        drainGroup.enter()
        DispatchQueue.global(qos: .userInitiated).async {
            defer { drainGroup.leave() }
            while true {
                let data = handle.availableData
                guard !data.isEmpty else { break }
                collector.consume(data)
            }
        }
    }

    process.waitUntilExit()
    drainGroup.wait()
    return ProcessOutputResult(
        terminationStatus: process.terminationStatus,
        data: collector.finish(remainingData: [])
    )
}
