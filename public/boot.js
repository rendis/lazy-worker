// Runs before the first paint, so the panel never shows the light theme for an instant on a dark Mac.
// The app saves the chosen theme here. Without a saved choice, the system decides.
(function () {
  var saved = null;
  try {
    saved = localStorage.getItem("lazy-worker-theme");
  } catch (error) {
    // Storage can be unavailable. The system theme is the answer then.
  }
  var dark = saved === "dark" || (saved !== "light" && matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
})();
