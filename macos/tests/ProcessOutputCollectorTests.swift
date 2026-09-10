import Foundation

@main
struct ProcessOutputCollectorTests {
    static func main() throws {
        let chunkSize = 2 * 1_024 * 1_024
        let process = Process()
        let outputPipe = Pipe()
        let errorPipe = Pipe()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = [
            "-c",
            "printf '{\"ok\":true}'; printf 'models.dev warning\\n' >&2; yes o | head -c \(chunkSize); yes e | head -c \(chunkSize) >&2",
        ]
        process.standardOutput = outputPipe
        process.standardError = errorPipe

        let result = try runProcessCollectingOutput(
            process,
            outputPipe: outputPipe,
            errorPipe: errorPipe
        )

        precondition(result.terminationStatus == 0)
        precondition(
            String(decoding: result.stdoutData.prefix(11), as: UTF8.self) == "{\"ok\":true}",
            "stdout should remain a standalone JSON document"
        )
        precondition(
            String(decoding: result.stderrData.prefix(19), as: UTF8.self)
                == "models.dev warning\n",
            "stderr should not contaminate stdout"
        )
        precondition(
            result.stdoutData.count == 11 + chunkSize,
            "collector should return complete stdout output"
        )
        precondition(
            result.stderrData.count == 19 + chunkSize,
            "collector should return complete stderr output"
        )

        let failedProcess = Process()
        let failedOutputPipe = Pipe()
        let failedErrorPipe = Pipe()
        failedProcess.executableURL = URL(fileURLWithPath: "/bin/sh")
        failedProcess.arguments = [
            "-c",
            "printf 'partial JSON'; printf 'upstream failure' >&2; exit 7",
        ]
        failedProcess.standardOutput = failedOutputPipe
        failedProcess.standardError = failedErrorPipe
        let failedResult = try runProcessCollectingOutput(
            failedProcess,
            outputPipe: failedOutputPipe,
            errorPipe: failedErrorPipe
        )
        precondition(failedResult.terminationStatus == 7)
        precondition(
            String(decoding: failedResult.stdoutData, as: UTF8.self) == "partial JSON"
        )
        precondition(
            String(decoding: failedResult.stderrData, as: UTF8.self) == "upstream failure"
        )
        precondition(processFailureMessage(failedResult) == "upstream failure\npartial JSON")

        let hangingProcess = Process()
        let hangingOutputPipe = Pipe()
        let hangingErrorPipe = Pipe()
        hangingProcess.executableURL = URL(fileURLWithPath: "/usr/bin/yes")
        hangingProcess.standardOutput = hangingOutputPipe
        hangingProcess.standardError = hangingErrorPipe
        let timeoutStarted = Date()
        let timedOutResult = try runProcessCollectingOutput(
            hangingProcess,
            outputPipe: hangingOutputPipe,
            errorPipe: hangingErrorPipe,
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
        streamingCollector.finish(remainingData: [
            Data("ond\nMIXIN_PROGRESS third".utf8),
        ])
        progressQueue.sync {}

        precondition(
            progressLines == ["first", "second", "third"]
        )

        let streamingProcess = Process()
        let streamingOutputPipe = Pipe()
        let streamingErrorPipe = Pipe()
        streamingProcess.executableURL = URL(fileURLWithPath: "/bin/sh")
        streamingProcess.arguments = [
            "-c",
            "printf 'command result\\n'; yes o | head -c \(chunkSize); printf '\\nMIXIN_PROGRESS stdout-ignored\\n'; yes e | head -c \(chunkSize) >&2; printf '\\nMIXIN_PROGRESS stderr-done\\n' >&2",
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
        precondition(
            String(decoding: streamingResult.stdoutData.prefix(15), as: UTF8.self)
                == "command result\n"
        )
        precondition(streamingResult.stderrData.count > chunkSize)
        precondition(processProgressLines == ["stderr-done"])
        precondition(
            !processProgressLines.contains("stdout-ignored"),
            "stdout must not be interpreted as progress"
        )
        print("Process output collector tests passed")
    }
}
