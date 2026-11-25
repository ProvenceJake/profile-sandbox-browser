import Foundation

enum ViewerError: LocalizedError {
  case message(String)
  var errorDescription: String? {
    if case .message(let text) = self { return text }
    return nil
  }
}

typealias Fields = [String: Any]
@MainActor final class Connection {
  let session: URLSession
  let socket: URLSessionWebSocketTask
  var target = ""
  var sequence = 0
  var pending = [Int: CheckedContinuation<Fields, Error>]()
  var failure: Error?
  init(port: Int, token: String) {
    let config = URLSessionConfiguration.ephemeral
    config.connectionProxyDictionary = [:]
    session = URLSession(configuration: config)
    var request = URLRequest(url: URL(string: "ws://127.0.0.1:\(port)/devtools/browser")!)
    request.setValue("Bearer " + token, forHTTPHeaderField: "Authorization")
    socket = session.webSocketTask(with: request)
    socket.maximumMessageSize = 16 * 1024 * 1024
    socket.resume()
    Task { await receive() }
  }
  func close() {
    socket.cancel(with: .goingAway, reason: nil)
    session.invalidateAndCancel()
    fail(ViewerError.message("内核连接已关闭"))
  }
  private func fail(_ err: Error) {
    failure = err
    let requests = pending
    pending.removeAll()
    for handler in requests.values { handler.resume(throwing: err) }
  }
  private func receive() async {
    do {
      while true {
        let msg = try await socket.receive()
        let data: Data
        switch msg {
        case .data(let d): data = d
        case .string(let s): data = Data(s.utf8)
        @unknown default: continue
        }
        guard let obj = try JSONSerialization.jsonObject(with: data) as? Fields,
          let id = obj["id"] as? Int, let reply = pending.removeValue(forKey: id)
        else { continue }
        if let error = obj["error"] as? Fields {
          reply.resume(throwing: ViewerError.message(error["message"] as? String ?? "内核指令失败"))
        } else {
          reply.resume(returning: obj["result"] as? Fields ?? [:])
        }
      }
    } catch { fail(error) }
  }
  @discardableResult func call(_ method: String, _ params: Fields = [:], page: Bool = true)
    async throws -> Fields
  {
    if let error = failure { throw error }
    sequence += 1
    let id = sequence
    var body: Fields = ["id": id, "method": method, "params": params]
    if page { body["sessionId"] = target }
    let encoded = String(decoding: try JSONSerialization.data(withJSONObject: body), as: UTF8.self)
    return try await withCheckedThrowingContinuation { continuation in
      pending[id] = continuation
      Task {
        do { try await socket.send(.string(encoded)) } catch {
          pending.removeValue(forKey: id)?.resume(throwing: error)
        }
      }
      Task {
        try? await Task.sleep(nanoseconds: 15_000_000_000)
        if let late = pending.removeValue(forKey: id) {
          late.resume(throwing: ViewerError.message("页面响应超时"))
        }
      }
    }
  }
  func evaluate(_ expression: String) async throws -> Any? {
    let r = try await call("Runtime.evaluate", ["expression": expression, "returnByValue": true])
    if r["exceptionDetails"] != nil { throw ViewerError.message("读取页面状态失败") }
    return (r["result"] as? Fields)?["value"]
  }
  func navigate(_ address: String) async throws {
    let result = try await call("Page.navigate", ["url": address])
    if let text = result["errorText"] as? String, !text.isEmpty { throw ViewerError.message(text) }
  }
}

struct LaunchSettings: Decodable {
  let name, zone, locale, echo, ip, directory, binary, guardBinary: String
  let port: Int
}

// The guard watches the viewer's pipe and terminates only the recorded engine PID.
@MainActor final class Engine {
  let process = Process(), guardTask = Process(), guardPipe = Pipe()
  let temp = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
  var stopped = false
  var connection: Connection?
  func start(_ cfg: LaunchSettings) async throws -> Connection {
    try FileManager.default.createDirectory(
      at: temp, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
    let ready = temp.appendingPathComponent("ready")
    let token = UUID().uuidString + UUID().uuidString
    let policy =
      "(version 1)(allow default)(deny network-outbound)(allow network-outbound (remote ip \"localhost:\(cfg.port)\"))"
    process.executableURL = URL(fileURLWithPath: "/usr/bin/sandbox-exec")
    process.arguments = [
      "-p", policy, cfg.binary, "serve", "--host", "127.0.0.1", "--port", "0", "--ready-file",
      ready.path, "--proxy", "http://127.0.0.1:\(cfg.port)", "--storage-dir", cfg.directory,
    ]
    var env = ProcessInfo.processInfo.environment
    for key in env.keys.filter({
      $0.hasPrefix("OBSCURA_")
        || ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"].contains($0.uppercased())
    }) { env.removeValue(forKey: key) }
    env["OBSCURA_CDP_TOKEN"] = token
    env["TZ"] = cfg.zone
    process.environment = env
    process.standardInput = FileHandle.nullDevice
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    try process.run()
    guardTask.executableURL = URL(fileURLWithPath: cfg.guardBinary)
    guardTask.arguments = [String(process.processIdentifier)]
    guardTask.standardInput = guardPipe
    guardTask.standardOutput = FileHandle.nullDevice
    try guardTask.run()
    for _ in 0..<100 {
      if !process.isRunning { throw ViewerError.message("Obscura 启动失败") }
      if let data = try? Data(contentsOf: ready),
        let obj = try? JSONSerialization.jsonObject(with: data) as? Fields,
        obj["pid"] as? Int32 == process.processIdentifier, let port = obj["port"] as? Int,
        (1...65535).contains(port)
      {
        let c = Connection(port: port, token: token)
        connection = c
        let target = try await c.call("Target.createTarget", ["url": "about:blank"], page: false)
        guard let id = target["targetId"] as? String else { throw ViewerError.message("无法创建页面") }
        let attached = try await c.call(
          "Target.attachToTarget", ["targetId": id, "flatten": true], page: false)
        guard let session = attached["sessionId"] as? String else {
          throw ViewerError.message("无法连接页面")
        }
        c.target = session
        try await c.call("Emulation.setLocaleOverride", ["locale": cfg.locale])
        try await c.call(
          "Emulation.setUserAgentOverride", ["userAgent": "", "acceptLanguage": cfg.locale])
        return c
      }
      try await Task.sleep(nanoseconds: 50_000_000)
    }
    throw ViewerError.message("等待 Obscura 启动超时")
  }
  func stop() {
    guard !stopped else { return };stopped = true
    connection?.close()
    if process.isRunning { process.terminate() }
    try? guardPipe.fileHandleForWriting.close()
    if guardTask.isRunning { guardTask.waitUntilExit() }
    if process.isRunning { process.waitUntilExit() }
    try? FileManager.default.removeItem(at: temp)
  }
}
