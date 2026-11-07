import Foundation
import XCTest

/// `URLProtocol` fixture that hands every request to a test-installed
/// handler instead of the network. Register it on an ephemeral
/// `URLSessionConfiguration` and inject that session into the API client
/// so responses, status codes and transport failures are deterministic.
final class StubURLProtocol: URLProtocol {
    /// Set by each test before making a call. Returning a transport
    /// `Error` (e.g. `URLError`) simulates an unreachable daemon.
    static var requestHandler: ((URLRequest) throws -> (HTTPURLResponse, Data))?

    override class func canInit(with request: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        guard let handler = StubURLProtocol.requestHandler else {
            client?.urlProtocol(
                self,
                didFailWithError: NSError(
                    domain: "StubURLProtocol",
                    code: -1,
                    userInfo: [NSLocalizedDescriptionKey: "requestHandler was not installed"]
                )
            )
            return
        }
        do {
            let (response, data) = try handler(request)
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            client?.urlProtocol(self, didFailWithError: error)
        }
    }

    override func stopLoading() {}
}
