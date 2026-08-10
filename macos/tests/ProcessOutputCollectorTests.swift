import Foundation

@main
struct ProcessOutputCollectorTests {
    static func main() throws {
        let chunkSize = 2 * 1_024 * 1_024
        let process = Process()
        let outputPipe = Pipe()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = [
            "-c",
            "yes o | head -c \(chunkSize); yes e | head -c \(chunkSize) >&2",
        ]
        process.standardOutput = outputPipe
        process.standardError = outputPipe

        let result = try runProcessCollectingMergedOutput(
            process,
            outputPipe: outputPipe
        )

        precondition(result.terminationStatus == 0)
        precondition(
            result.data.count == chunkSize * 2,
            "collector should return complete stdout and stderr output"
        )

        let hangingProcess = Process()
        let hangingPipe = Pipe()
        hangingProcess.executableURL = URL(fileURLWithPath: "/usr/bin/yes")
        hangingProcess.standardOutput = FileHandle.nullDevice
        hangingProcess.standardError = hangingPipe
        let timeoutStarted = Date()
        let timedOutResult = try runProcessCollectingMergedOutput(
            hangingProcess,
            outputPipe: hangingPipe,
            timeout: 1,
            killGrace: 1
        )
        precondition(timedOutResult.terminationStatus != 0)
        precondition(
            Date().timeIntervalSince(timeoutStarted) < 20,
            "hard timeout should stop a hung child process"
        )

        let progressQueue = DispatchQueue(label: "process-output-progress-test")
        var progressLines: [String] = []
        let streamingCollector = StreamingProcessOutputCollector(
            progressPrefix: "MIXIN_PROGRESS ",
            progressQueue: progressQueue
        ) { line in
            progressLines.append(line)
        }
        streamingCollector.consume(Data("MIXIN_PRO".utf8))
        streamingCollector.consume(Data("GRESS first\nignored\nMIXIN_PROGRESS sec".utf8))
        let streamedData = streamingCollector.finish(remainingData: [
            Data("ond\nMIXIN_PROGRESS third".utf8),
        ])
        progressQueue.sync {}

        precondition(
            String(decoding: streamedData, as: UTF8.self)
                == "MIXIN_PROGRESS first\nignored\nMIXIN_PROGRESS second\nMIXIN_PROGRESS third"
        )
        precondition(progressLines == ["first", "second", "third"])

        let streamingProcess = Process()
        let streamingOutputPipe = Pipe()
        let streamingErrorPipe = Pipe()
        streamingProcess.executableURL = URL(fileURLWithPath: "/bin/sh")
        streamingProcess.arguments = [
            "-c",
            "yes o | head -c \(chunkSize); printf '\\nMIXIN_PROGRESS stdout-done\\n'; yes e | head -c \(chunkSize) >&2; printf '\\nMIXIN_PROGRESS stderr-done\\n' >&2",
        ]
        streamingProcess.standardOutput = streamingOutputPipe
        streamingProcess.standardError = streamingErrorPipe
        var processProgressLines: [String] = []
        let processCollector = StreamingProcessOutputCollector(
            progressPrefix: "MIXIN_PROGRESS ",
            progressQueue: progressQueue
        ) { line in
            processProgressLines.append(line)
        }
        let streamingResult = try runProcessCollectingStreamingOutput(
            streamingProcess,
            outputPipe: streamingOutputPipe,
            errorPipe: streamingErrorPipe,
            collector: processCollector
        )
        progressQueue.sync {}

        precondition(streamingResult.terminationStatus == 0)
        precondition(streamingResult.data.count > chunkSize * 2)
        precondition(Set(processProgressLines) == ["stdout-done", "stderr-done"])
        print("Process output collector tests passed")
    }
}
