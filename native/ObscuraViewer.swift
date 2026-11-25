import AppKit

@MainActor final class PageSurface: NSView, @preconcurrency NSTextInputClient {
  private var composition = ""
  private var insertionPoint = NSPoint.zero
  var landingText: String? { didSet { needsDisplay = true } }
  var image: NSImage? { didSet { needsDisplay = true } }
  weak var viewer: Viewer?
  override var isFlipped: Bool { true }
  override var acceptsFirstResponder: Bool { true }
  override func draw(_ dirtyRect: NSRect) {
    NSColor.white.setFill()
    bounds.fill()
    image?.draw(
      in: bounds, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil
    )
    if let landingText {
      let style = NSMutableParagraphStyle()
      style.alignment = .center
      (landingText as NSString).draw(
        in: NSRect(x: 20, y: bounds.midY - 30, width: max(1, bounds.width - 40), height: 62),
        withAttributes: [
          .paragraphStyle: style, .font: NSFont.systemFont(ofSize: 18, weight: .medium),
          .foregroundColor: NSColor.darkGray,
        ])
    }
    if !composition.isEmpty {
      (composition as NSString).draw(
        at: insertionPoint,
        withAttributes: [
          .backgroundColor: NSColor.textBackgroundColor, .foregroundColor: NSColor.textColor,
          .underlineStyle: NSUnderlineStyle.single.rawValue,
        ])
    }
  }
  func hasMarkedText() -> Bool { !composition.isEmpty }
  func markedRange() -> NSRange {
    composition.isEmpty
      ? NSRange(location: NSNotFound, length: 0)
      : NSRange(location: 0, length: (composition as NSString).length)
  }
  func selectedRange() -> NSRange { NSRange(location: (composition as NSString).length, length: 0) }
  func setMarkedText(_ value: Any, selectedRange: NSRange, replacementRange: NSRange) {
    composition = (value as? NSAttributedString)?.string ?? (value as? String ?? "")
    needsDisplay = true
  }
  func unmarkText() {
    composition = ""
    needsDisplay = true
  }
  func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }
  func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?)
    -> NSAttributedString?
  { nil }
  func characterIndex(for point: NSPoint) -> Int { NSNotFound }
  func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
    let r = convert(NSRect(origin: insertionPoint, size: NSSize(width: 1, height: 18)), to: nil)
    return window?.convertToScreen(r) ?? .zero
  }
  func insertText(_ value: Any, replacementRange: NSRange) { insertText(value) }
  private func mouse(_ event: NSEvent, _ type: String) {
    guard let viewer, image != nil, bounds.width > 0, bounds.height > 0 else { return }
    let p = convert(event.locationInWindow, from: nil)
    var args: Fields = [
      "type": type, "x": p.x * viewer.viewport.width / bounds.width,
      "y": p.y * viewer.viewport.height / bounds.height,
    ]
    if type == "mouseWheel" {
      args["deltaX"] = -event.scrollingDeltaX
      args["deltaY"] = -event.scrollingDeltaY
    } else {
      args["button"] = "left"
      args["clickCount"] = event.clickCount
    }
    viewer.input("Input.dispatchMouseEvent", args)
  }
  override func mouseDown(with event: NSEvent) {
    insertionPoint = convert(event.locationInWindow, from: nil)
    window?.makeFirstResponder(self)
    mouse(event, "mousePressed")
  }
  override func mouseUp(with event: NSEvent) { mouse(event, "mouseReleased") }
  override func scrollWheel(with event: NSEvent) { mouse(event, "mouseWheel") }
  override func keyDown(with event: NSEvent) { interpretKeyEvents([event]) }
  override func insertText(_ value: Any) {
    unmarkText()
    let text = (value as? NSAttributedString)?.string ?? (value as? String ?? "")
    if !text.isEmpty { viewer?.input("Input.insertText", ["text": text]) }
  }
  override func doCommand(by selector: Selector) {
    let keys: [String: (String, Int)] = [
      "deleteBackward:": ("Backspace", 8), "deleteForward:": ("Delete", 46),
      "insertNewline:": ("Enter", 13), "insertTab:": ("Tab", 9), "moveLeft:": ("ArrowLeft", 37),
      "moveRight:": ("ArrowRight", 39), "moveUp:": ("ArrowUp", 38), "moveDown:": ("ArrowDown", 40),
      "cancelOperation:": ("Escape", 27),
    ]
    if let (name, code) = keys[NSStringFromSelector(selector)] {
      for type in ["rawKeyDown", "keyUp"] {
        viewer?.input(
          "Input.dispatchKeyEvent", ["type": type, "key": name, "windowsVirtualKeyCode": code])
      }
    } else {
      super.doCommand(by: selector)
    }
  }
  @objc func paste(_ sender: Any?) {
    if let text = NSPasteboard.general.string(forType: .string) { insertText(text) }
  }
}

@MainActor final class Viewer: NSObject, NSWindowDelegate {
  let config: LaunchSettings, c: Connection
  let window = NSWindow(
    contentRect: NSRect(x: 0, y: 0, width: 950, height: 690),
    styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
  let address = NSTextField(), surface = PageSurface(), status = NSTextField(labelWithString: "")
  let back = NSButton(), forward = NSButton(), reload = NSButton()
  var viewport = NSSize(width: 900, height: 600)
  private var history = [String](), index = -1, lastURL = "", busy = false
  private var queued: Task<Void, Never>?
  private var renderLoop: Task<Void, Never>?
  init(_ config: LaunchSettings, _ c: Connection) {
    self.config = config
    self.c = c
    super.init()
    window.title = config.name + " · Obscura"
    window.minSize = NSSize(width: 460, height: 320)
    window.delegate = self
    window.isReleasedWhenClosed = false
    let base = NSView()
    window.contentView = base
    for (button, symbol, title, action) in [
      (back, "chevron.left", "后退", #selector(previous)),
      (forward, "chevron.right", "前进", #selector(next)),
      (reload, "arrow.clockwise", "刷新", #selector(refresh)),
    ] {
      button.bezelStyle = .rounded
      button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: title)
      button.toolTip = title
      button.target = self
      button.action = action
    }
    address.placeholderString = "输入网址"
    address.target = self
    address.action = #selector(go)
    address.lineBreakMode = .byTruncatingMiddle
    status.font = .systemFont(ofSize: 11)
    status.textColor = .secondaryLabelColor
    status.lineBreakMode = .byTruncatingMiddle
    let bar = NSStackView(views: [back, forward, reload, address])
    bar.spacing = 6
    for view in [bar, surface, status] {
      view.translatesAutoresizingMaskIntoConstraints = false
      base.addSubview(view)
    }
    NSLayoutConstraint.activate([
      bar.leadingAnchor.constraint(equalTo: base.leadingAnchor, constant: 10),
      bar.trailingAnchor.constraint(equalTo: base.trailingAnchor, constant: -10),
      bar.topAnchor.constraint(equalTo: base.topAnchor, constant: 8),
      surface.leadingAnchor.constraint(equalTo: base.leadingAnchor),
      surface.trailingAnchor.constraint(equalTo: base.trailingAnchor),
      surface.topAnchor.constraint(equalTo: bar.bottomAnchor, constant: 8),
      surface.bottomAnchor.constraint(equalTo: status.topAnchor, constant: -4),
      status.leadingAnchor.constraint(equalTo: base.leadingAnchor, constant: 10),
      status.trailingAnchor.constraint(equalTo: base.trailingAnchor, constant: -10),
      status.bottomAnchor.constraint(equalTo: base.bottomAnchor, constant: -4),
    ])
    surface.viewer = self
    surface.landingText = config.name + "\n输入网址，按回车开始浏览"
    updateButtons()
    normalStatus()
  }
  func normalStatus() {
    status.stringValue =
      "Obscura  ·  " + config.ip + "  ·  " + config.zone + "  ·  " + config.locale
  }
  func updateButtons() {
    back.isEnabled = index > 0
    forward.isEnabled = index + 1 < history.count
    reload.isEnabled = index >= 0
  }
  func show() {
    window.center()
    window.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)
    window.makeFirstResponder(address)
    renderLoop = Task { [weak self] in
      while !Task.isCancelled {
        if let self, self.window.isVisible, !self.window.isMiniaturized, !self.busy,
          self.queued == nil
        {
          self.enqueue { await self.render() }
        }
        try? await Task.sleep(nanoseconds: 800_000_000)
      }
    }
  }
  func windowWillClose(_ notification: Notification) {
    renderLoop?.cancel()
    NSApp.terminate(nil)
  }
  @objc func focusAddress() {
    window.makeFirstResponder(address)
    address.selectText(nil)
  }
  @objc func go() {
    var text = address.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
    if !text.contains(":")
      || (!text.contains("://")
        && text.range(of: #"^[\w.-]+:\d+(/|$)"#, options: .regularExpression) != nil)
    {
      text = "https://" + text
    }
    guard let u = URL(string: text), ["https", "http"].contains(u.scheme?.lowercased() ?? ""),
      u.host != nil, u.user == nil, u.password == nil
    else {
      status.stringValue = "请输入 HTTP 或 HTTPS 网址"
      return
    }
    load(u.absoluteString)
  }
  @objc func previous() { if index > 0 { load(history[index - 1], position: index - 1) } }
  @objc func next() {
    if index + 1 < history.count { load(history[index + 1], position: index + 1) }
  }
  @objc func refresh() { if index >= 0 { load(history[index], position: index) } }
  private func enqueue(redraw: Bool = false, _ block: @escaping () async throws -> Void) {
    let prior = queued
    let ticket = UUID()
    latest = ticket
    queued = Task {
      await prior?.value
      do { try await block() } catch { self.status.stringValue = error.localizedDescription }
      if self.latest == ticket && redraw { await self.render() }
      if self.latest == ticket {
        self.queued = nil
      }
    }
  }
  private var latest = UUID()
  private func load(_ url: String, position: Int? = nil) {
    enqueue {
      self.status.stringValue = "正在加载…"
      try await self.c.navigate(url)
      if let position { self.index = position }
      self.lastURL = position == nil ? "" : url
      self.window.makeFirstResponder(self.surface)
      self.normalStatus()
      await self.render()
    }
  }
  func input(_ method: String, _ params: Fields) {
    enqueue(redraw: true) { try await self.c.call(method, params) }
  }
  private func render() async {
    guard !busy else { return }
    busy = true
    defer { busy = false }
    do {
      let size = NSSize(
        width: max(100, min(2048, Int(surface.bounds.width))),
        height: max(100, min(1536, Int(surface.bounds.height))))
      if size != viewport || surface.image == nil {
        try await c.call(
          "Emulation.setDeviceMetricsOverride",
          [
            "width": Int(size.width), "height": Int(size.height), "deviceScaleFactor": 1,
            "mobile": false,
          ])
        viewport = size
      }
      let shot = try await c.call("Page.captureScreenshot", ["format": "png"])
      guard let payload = shot["data"] as? String, let data = Data(base64Encoded: payload),
        let img = NSImage(data: data)
      else { throw ViewerError.message("内核未返回有效画面") }
      surface.image = img
      if let url = try await c.evaluate("location.href") as? String,
        url.hasPrefix("https://") || url.hasPrefix("http://")
      {
        surface.landingText = nil
        if url != lastURL {
          history = Array(history.prefix(index + 1))
          if history.last != url { history.append(url) }
          index = history.count - 1
          lastURL = url
          updateButtons()
        }
        if address.currentEditor() == nil { address.stringValue = url }
      }
      updateButtons()
    } catch { status.stringValue = error.localizedDescription }
  }
}

@MainActor final class Delegate: NSObject, NSApplicationDelegate {
  let engine = Engine()
  var viewer: Viewer?
  func applicationDidFinishLaunching(_ notification: Notification) {
    Task {
      do {
        guard let line = readLine() else { throw ViewerError.message("缺少启动配置") }
        let cfg = try JSONDecoder().decode(LaunchSettings.self, from: Data(line.utf8))
        let c = try await engine.start(cfg)
        try await c.navigate(cfg.echo)
        let body = try await c.evaluate("document.body.textContent") as? String ?? ""
        let zone =
          try await c.evaluate("Intl.DateTimeFormat().resolvedOptions().timeZone") as? String ?? ""
        let language = try await c.evaluate("navigator.language") as? String ?? ""
        let expectedZone =
          try await c.evaluate(
            "new Intl.DateTimeFormat('en',{timeZone:"
              + String(decoding: try JSONEncoder().encode(cfg.zone), as: UTF8.self)
              + "}).resolvedOptions().timeZone") as? String
        guard zone == expectedZone, language.caseInsensitiveCompare(cfg.locale) == .orderedSame
        else { throw ViewerError.message("页面时区或首选语言校验失败") }
        try await c.navigate("about:blank")
        reply(["body": String(body.prefix(16384))])
        guard readLine() == "show" else { throw ViewerError.message("启动已取消") }
        DispatchQueue.global().async {
          _ = readLine()
          DispatchQueue.main.async { NSApp.terminate(nil) }
        }
        let w = Viewer(cfg, c)
        viewer = w
        installMenu(w)
        engine.process.terminationHandler = { [weak self] _ in
          Task { @MainActor in
            guard let self, !self.engine.stopped else { return }
            self.engine.stop()
            exit(1)
          }
        }
        w.show()
      } catch {
        reply(["error": error.localizedDescription])
        NSApp.terminate(nil)
      }
    }
  }
  func reply(_ obj: Fields) {
    if var data = try? JSONSerialization.data(withJSONObject: obj) {
      data.append(10)
      FileHandle.standardOutput.write(data)
    }
  }
  func applicationWillTerminate(_ notification: Notification) { engine.stop() }
  private func installMenu(_ w: Viewer) {
    let main = NSMenu()
    func add(_ name: String, _ entries: [(String, Selector, String, AnyObject?)]) {
      let top = NSMenuItem()
      main.addItem(top)
      let menu = NSMenu(title: name)
      top.submenu = menu
      for (text, action, key, target) in entries {
        let item = menu.addItem(withTitle: text, action: action, keyEquivalent: key)
        item.target = target
      }
    }
    add("Obscura", [("关闭窗口", #selector(NSWindow.performClose(_:)), "w", w.window)])
    add(
      "编辑",
      [
        ("剪切", #selector(NSText.cut(_:)), "x", nil), ("复制", #selector(NSText.copy(_:)), "c", nil),
        ("粘贴", #selector(NSText.paste(_:)), "v", nil),
        ("全选", #selector(NSResponder.selectAll(_:)), "a", nil),
      ])
    add(
      "浏览",
      [("地址栏", #selector(Viewer.focusAddress), "l", w), ("刷新", #selector(Viewer.refresh), "r", w)])
    NSApp.mainMenu = main
  }
}
#if !VIEWER_TEST
  @main struct ViewerMain {
    @MainActor static func main() {
      let app = NSApplication.shared
      let delegate = Delegate()
      app.setActivationPolicy(.regular)
      app.delegate = delegate
      withExtendedLifetime(delegate) { app.run() }
    }
  }

#endif
