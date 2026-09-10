import Foundation
import Darwin

struct ProcessOutputResult {
    let terminationStatus: Int32
    let stdoutData: Data
    let stderrData: Data
}

func processFailureMessage(_ result: ProcessOutputResult) -> String {
    let stderr = String(decoding: result.stderrData, as: UTF8.self)
        .trimmingCharacters(in: .whitespacesAndNewlines)
    let stdout = String(decoding: result.stdoutData, as: UTF8.self)
        .trimmingCharacters(in: .whitespacesAndNewlines)
    let output = [stderr, stdout].filter { !$0.isEmpty }
    return output.isEmpty ? "exit \(result.terminationStatus)" : output.joined(separator: "\n")
}

private final class DataAccumulator {
    private let lock = NSLock()
    private var data = Data()

    func append(_ newData: Data) {
        lock.lock()
        data.append(newData)
        lock.unlock()
    }

    var value: Data {
        lock.lock()
        defer { lock.unlock() }
        return data
    }
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

func runProcessCollectingOutput(
    _ process: Process,
    outputPipe: Pipe,
    errorPipe: Pipe,
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

    let stdout = DataAccumulator()
    let stderr = DataAccumulator()
    let drainGroup = DispatchGroup()
    // Drain both independent pipes concurrently so either stream can exceed
    // the OS pipe buffer without blocking the child process.
    for (handle, accumulator) in [
        (outputPipe.fileHandleForReading, stdout),
        (errorPipe.fileHandleForReading, stderr),
    ] {
        drainGroup.enter()
        DispatchQueue.global(qos: .userInitiated).async {
            defer { drainGroup.leave() }
            while true {
                let data = handle.availableData
                guard !data.isEmpty else { break }
                accumulator.append(data)
            }
        }
    }

    process.waitUntilExit()
    drainGroup.wait()

    return ProcessOutputResult(
        terminationStatus: process.terminationStatus,
        stdoutData: stdout.value,
        stderrData: stderr.value
    )
}

final class StreamingProcessOutputCollector {
    private let queue = DispatchQueue(label: "local.codex-mixin.streaming-process-output")
    private let progressQueue: DispatchQueue
    private let onProgress: (String) -> Void
    private let progressPrefix: String?
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

    func finish(remainingData: [Data]) {
        queue.sync {
            for data in remainingData where !data.isEmpty {
                consumeOnQueue(data)
            }
            emitProgressOnQueue(pendingBuffer)
            pendingBuffer = ""
        }
    }

    private func consumeOnQueue(_ data: Data) {
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

    let stdout = DataAccumulator()
    let stderr = DataAccumulator()
    let drainGroup = DispatchGroup()
    // Streaming keeps progress on stderr while stdout remains the command result.
    drainGroup.enter()
    DispatchQueue.global(qos: .userInitiated).async {
        defer { drainGroup.leave() }
        while true {
            let data = outputPipe.fileHandleForReading.availableData
            guard !data.isEmpty else { break }
            stdout.append(data)
        }
    }
    drainGroup.enter()
    DispatchQueue.global(qos: .userInitiated).async {
        defer { drainGroup.leave() }
        while true {
            let data = errorPipe.fileHandleForReading.availableData
            guard !data.isEmpty else { break }
            stderr.append(data)
            collector.consume(data)
        }
    }

    process.waitUntilExit()
    drainGroup.wait()
    collector.finish(remainingData: [])
    return ProcessOutputResult(
        terminationStatus: process.terminationStatus,
        stdoutData: stdout.value,
        stderrData: stderr.value
    )
}
