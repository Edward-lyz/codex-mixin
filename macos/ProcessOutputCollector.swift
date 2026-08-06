import Foundation

struct ProcessOutputResult {
    let terminationStatus: Int32
    let data: Data
}

func runProcessCollectingMergedOutput(
    _ process: Process,
    outputPipe: Pipe
) throws -> ProcessOutputResult {
    try process.run()

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
