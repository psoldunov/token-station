import Foundation
import Testing

@testable import TokenStationCore

@Suite("Line framing")
struct LineFramerTests {
    @Test("a line arriving in pieces is put back together")
    func reassemblesSplitLines() throws {
        var framer = LineFramer()
        let partial = try framer.append(Data("{\"a\":".utf8))
        #expect(partial.isEmpty)

        let lines = try framer.append(Data("1}\n".utf8))
        #expect(lines.count == 1)
        #expect(String(bytes: lines[0], encoding: .utf8) == "{\"a\":1}")
        #expect(framer.pendingByteCount == 0)
    }

    @Test("several lines in one read come out in order")
    func splitsSeveralLines() throws {
        var framer = LineFramer()
        let lines = try framer.append(Data("one\ntwo\nthree".utf8))
        let texts = lines.map { String(bytes: $0, encoding: .utf8) }
        #expect(texts == ["one", "two"])
        #expect(framer.pendingByteCount == 5)
    }

    @Test("an empty line is a line, not a silence")
    func keepsEmptyLines() throws {
        var framer = LineFramer()
        let lines = try framer.append(Data("\n\n".utf8))
        #expect(lines.count == 2)
        let allEmpty = lines.allSatisfy(\.isEmpty)
        #expect(allEmpty)
    }

    @Test("a line past the protocol's limit is refused")
    func refusesOversizedLines() {
        var framer = LineFramer()
        let oversized = Data(
            repeating: UInt8(ascii: "x"), count: LineFramer.maximumLineLength + 1)
        var thrown: (any Error)?
        do {
            _ = try framer.append(oversized)
        } catch {
            thrown = error
        }
        #expect(thrown is LineFramer.LineTooLongError)
        #expect(framer.pendingByteCount == 0)
    }
}
