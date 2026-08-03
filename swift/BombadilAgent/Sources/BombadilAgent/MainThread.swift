#if os(macOS)

import AppKit

/// Runs work on the main thread even while it is nested in a menu or
/// modal tracking run loop.
///
/// `DispatchQueue.main.sync` only drains in the default run-loop mode,
/// so a tap that opens an NSMenu (which spins the run loop in the
/// event-tracking mode until the menu closes) would deadlock the agent.
/// Blocks scheduled in the *common* run-loop modes execute in tracking
/// and modal-panel modes too, and a timeout keeps a truly blocked main
/// thread (e.g. one stuck in a computation) from wedging the test.
enum MainThread {

    private final class Operation<T: Sendable>: @unchecked Sendable {
        private enum State {
            case pending
            case running
            case finished(Result<T, Error>)
            case cancelled
        }

        private let lock = NSLock()
        private let completion = DispatchSemaphore(value: 0)
        private var state = State.pending

        func begin() -> Bool {
            lock.lock()
            defer { lock.unlock() }
            guard case .pending = state else {
                return false
            }
            state = .running
            return true
        }

        func finish(_ result: Result<T, Error>) {
            lock.lock()
            state = .finished(result)
            lock.unlock()
            completion.signal()
        }

        /// Cancel only work that has not begun. Once main-thread work is
        /// running it cannot be safely interrupted, so the caller waits
        /// for its result instead of reporting a timeout while it carries on.
        func cancelIfPending() -> Bool {
            lock.lock()
            defer { lock.unlock() }
            guard case .pending = state else {
                return false
            }
            state = .cancelled
            return true
        }

        func wait(timeout: TimeInterval) -> DispatchTimeoutResult {
            completion.wait(timeout: .now() + timeout)
        }

        func wait() {
            completion.wait()
        }

        func result() -> Result<T, Error>? {
            lock.lock()
            defer { lock.unlock() }
            guard case .finished(let result) = state else {
                return nil
            }
            return result
        }
    }

    static func run<T: Sendable>(
        timeout: TimeInterval,
        _ body: @MainActor @escaping @Sendable () throws -> T
    ) throws -> T {
        if Thread.isMainThread {
            return try MainActor.assumeIsolated {
                try body()
            }
        }
        let operation = Operation<T>()
        CFRunLoopPerformBlock(
            CFRunLoopGetMain(),
            CFRunLoopMode.commonModes.rawValue
        ) {
            guard operation.begin() else {
                return
            }
            let result = Result {
                try MainActor.assumeIsolated {
                    try body()
                }
            }
            operation.finish(result)
        }
        CFRunLoopWakeUp(CFRunLoopGetMain())
        if operation.wait(timeout: timeout) == .timedOut {
            if operation.cancelIfPending() {
                throw AgentError.actionFailed(
                    "main thread unavailable for \(timeout)s "
                        + "(blocked outside the run loop?)")
            }
            // The operation won the race and started before the timeout.
            // Waiting preserves request/reply ordering and prevents a late
            // action from running after the driver has observed an error.
            operation.wait()
        }
        guard let result = operation.result() else {
            throw AgentError.actionFailed(
                "main-thread operation completed without a result")
        }
        return try result.get()
    }
}

#endif
