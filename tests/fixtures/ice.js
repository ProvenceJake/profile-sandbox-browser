(async () => {
  const candidates = [];
  const peer = new RTCPeerConnection({
    iceServers: [{ urls: "stun:127.0.0.1:STUN_PORT" }],
  });
  peer.onicecandidate = (event) => {
    if (event.candidate) candidates.push(event.candidate.candidate);
  };
  peer.createDataChannel("probe");
  try {
    await peer.setLocalDescription(await peer.createOffer());
    // A deliberately unreachable STUN service can keep Chrome in "gathering".
    // Observe the socket and all candidates throughout a fixed window.
    await new Promise((resolve) => setTimeout(resolve, 5000));
    if (!peer.localDescription?.sdp.includes("ice-ufrag:"))
      throw new Error("ICE was not initialized");
    return candidates;
  } finally {
    peer.close();
  }
})();
