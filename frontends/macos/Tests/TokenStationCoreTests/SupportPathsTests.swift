import Foundation
import Testing

@testable import TokenStationCore

@Suite("Support paths")
struct SupportPathsTests {
    @Test("the socket lives under the bundle identifier by default")
    func resolvesTheDefaultSocket() {
        let path = SupportPaths.socketPath(environment: ["HOME": "/Users/someone"])
        #expect(path
            == "/Users/someone/Library/Application Support/dev.soldunov.TokenStation/daemon.sock")
    }

    @Test("the override wins, which is what keeps tests off a real daemon")
    func honoursTheOverride() {
        let path = SupportPaths.socketPath(
            environment: ["HOME": "/Users/someone", "TOKEN_STATION_SOCKET": "/tmp/x.sock"])
        #expect(path == "/tmp/x.sock")
    }

    @Test("a Unix socket path stays inside sockaddr_un's 104 bytes")
    func fitsInSockaddrUn() {
        let path = SupportPaths.socketPath(environment: ["HOME": "/Users/someone"])
        #expect(path.utf8.count < 104)
    }

    @Test("the config lives beside the socket, whatever XDG says")
    func resolvesTheConfig() {
        let path = SupportPaths.configPath(
            environment: ["HOME": "/Users/someone", "XDG_CONFIG_HOME": "/cfg"])
        #expect(path
            == "/Users/someone/Library/Application Support/dev.soldunov.TokenStation/config.toml")
    }

    @Test("the socket override does not move the config")
    func theSocketOverrideLeavesTheConfigAlone() {
        let environment = ["HOME": "/Users/someone", "TOKEN_STATION_SOCKET": "/tmp/x.sock"]
        #expect(SupportPaths.socketPath(environment: environment) == "/tmp/x.sock")
        #expect(SupportPaths.configPath(environment: environment)
            == "/Users/someone/Library/Application Support/dev.soldunov.TokenStation/config.toml")
    }

    @Test("the cache is under Library/Caches")
    func resolvesTheCacheDirectory() {
        // A directory URL, so its path carries the trailing separator.
        let url = SupportPaths.cacheDirectory(environment: ["HOME": "/Users/someone"])
        #expect(url.path(percentEncoded: false)
            == "/Users/someone/Library/Caches/dev.soldunov.TokenStation/")
    }

    @Test("an empty helper override counts as none")
    func ignoresAnEmptyHelperOverride() {
        #expect(SupportPaths.helperOverride(environment: ["TOKEN_STATION_HELPER": ""]) == nil)
        #expect(SupportPaths.helperOverride(environment: ["TOKEN_STATION_HELPER": "/bin/x"])
            == "/bin/x")
    }
}
