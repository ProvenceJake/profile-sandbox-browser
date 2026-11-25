import AppKit

@main struct InteractionCheck {
  @MainActor static func main() async throws {
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    let config = try JSONDecoder().decode(
      LaunchSettings.self, from: FileHandle.standardInput.readDataToEndOfFile())
    let engine = Engine()
    defer { engine.stop() }
    let c = try await engine.start(config)
    let viewer = Viewer(config, c)
    viewer.show()
    func wait(_ assertion: String) async throws {
      for _ in 0..<70 {
        if try await c.evaluate(assertion) as? Bool == true { return }
        try await Task.sleep(nanoseconds: 100_000_000)
      }
      throw ViewerError.message("Fixture assertion failed: " + assertion)
    }
    viewer.address.stringValue = config.echo
    viewer.go()
    try await wait("document.title === 'Fixture'")
    for _ in 0..<40 {
      if viewer.surface.image != nil { break }
      try await Task.sleep(nanoseconds: 100_000_000)
    }
    guard viewer.surface.image != nil else { throw ViewerError.message("No rendered image") }
    func click(_ x: CGFloat, _ y: CGFloat) throws {
      let pos = viewer.surface.convert(NSPoint(x: x, y: y), to: nil)
      for type: NSEvent.EventType in [.leftMouseDown, .leftMouseUp] {
        let event = NSEvent.mouseEvent(
          with: type, location: pos, modifierFlags: [], timestamp: 0,
          windowNumber: viewer.window.windowNumber, context: nil, eventNumber: 0, clickCount: 1,
          pressure: 1)!
        if type == .leftMouseDown {
          viewer.surface.mouseDown(with: event)
        } else {
          viewer.surface.mouseUp(with: event)
        }
      }
    }
    try click(65, 30)
    try await wait("document.activeElement.id === 'word'")
    viewer.surface.insertText("日本 Tokyo")
    try await wait("document.querySelector('#word').value === '日本 Tokyo'")
    try click(65, 90)
    try await wait("document.title === 'Clicked'")
    viewer.input(
      "Input.dispatchMouseEvent",
      ["type": "mouseWheel", "x": 60, "y": 160, "deltaY": 300, "deltaX": 0])
    try await wait(
      "document.documentElement.scrollTop > 0 || document.body.scrollTop > 0 || window.scrollY > 0")
    _ = try await c.evaluate(
      "localStorage.setItem('fixture','retained');document.cookie='test=retained; path=/';true")
    let second = config.echo + "/next"
    viewer.address.stringValue = second
    viewer.go()
    try await wait("location.pathname.endsWith('/next')")
    for _ in 0..<20 {
      if viewer.back.isEnabled { break }
      try await Task.sleep(nanoseconds: 100_000_000)
    }
    viewer.previous()
    try await wait("location.pathname === '/fixture'")
    viewer.next()
    try await wait("location.pathname.endsWith('/next')")
    viewer.refresh()
    try await wait("document.title === 'Fixture'")
    print(
      "PASS native viewport, mouse hit testing, Unicode input, click, scrolling, back and forward")
    viewer.window.orderOut(nil)
    engine.stop()
    let restored = Engine()
    defer { restored.stop() }
    let session = try await restored.start(config)
    try await session.navigate(config.echo)
    guard
      try await session.evaluate(
        "localStorage.getItem('fixture') === 'retained' && document.cookie.includes('test=retained')"
      ) as? Bool == true
    else {
      throw ViewerError.message("Cookie or local storage did not survive restart")
    }
    try await session.navigate(config.echo + "/disconnect")
    do {
      try await session.navigate(config.echo + "/must-not-load")
      throw ViewerError.message("Engine accepted navigation after proxy shutdown")
    } catch let e as ViewerError {
      if e.localizedDescription == "Engine accepted navigation after proxy shutdown" { throw e }
    }
    print("PASS persisted cookies and localStorage, navigation failed with the proxy offline")
  }
}
