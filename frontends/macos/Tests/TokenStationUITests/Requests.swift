/// A queue of calls a fake is waiting to answer.
///
/// The code under test calls `call(_:)` and suspends. The test takes the calls
/// in arrival order with `next()` and answers each one when it chooses, which is
/// what lets a test hold a response back, answer out of order, or refuse one,
/// without a timer anywhere.
actor Requests<Input: Sendable, Output: Sendable> {
    struct Pending: Sendable {
        let input: Input
        private let resume: @Sendable (Result<Output, any Error>) -> Void

        fileprivate init(
            input: Input,
            resume: @escaping @Sendable (Result<Output, any Error>) -> Void
        ) {
            self.input = input
            self.resume = resume
        }

        func succeed(_ output: Output) {
            resume(.success(output))
        }

        func fail(_ error: any Error) {
            resume(.failure(error))
        }
    }

    private var arrived: [Pending] = []
    private var waiters: [CheckedContinuation<Pending, Never>] = []

    /// Called by the fake session: suspends until the test answers.
    func call(_ input: Input) async throws -> Output {
        try await withCheckedThrowingContinuation { continuation in
            let pending = Pending(input: input) { continuation.resume(with: $0) }
            if waiters.isEmpty {
                arrived.append(pending)
            } else {
                waiters.removeFirst().resume(returning: pending)
            }
        }
    }

    /// The oldest call nobody has taken yet, waiting for one if there is none.
    func next() async -> Pending {
        if !arrived.isEmpty { return arrived.removeFirst() }
        return await withCheckedContinuation { waiters.append($0) }
    }

    /// How many calls have arrived that no `next()` has taken.
    var pendingCount: Int { arrived.count }
}

extension Requests.Pending where Output == Void {
    func succeed() {
        succeed(())
    }
}
