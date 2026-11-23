(() => {
  if (window !== window.top || !["http:", "https:"].includes(location.protocol))
    return;
  const prefix = "[" + PROFILE_NAME + "] ";
  function update() {
    if (document.head && !document.title.startsWith(prefix))
      document.title = prefix + document.title;
  }
  document.addEventListener(
    "DOMContentLoaded",
    () => {
      update();
      if (document.head)
        new MutationObserver(update).observe(document.head, {
          subtree: true,
          childList: true,
          characterData: true,
        });
    },
    { once: true },
  );
})();
