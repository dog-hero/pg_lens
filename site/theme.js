// Landing/docs theme toggle. Deliberately tiny and dependency-free, and
// deliberately sharing the dashboard's storage key + dark-by-default rule
// (see crates/pg_lens_web/frontend/src/theme.ts) so the choice follows the
// visitor from the site into the demo.
(function () {
  var KEY = "pg_lens_theme";
  var root = document.documentElement;
  var icon = document.getElementById("theme-icon");
  var button = document.getElementById("theme-toggle");

  function paint(theme) {
    root.dataset.theme = theme;
    if (icon) icon.setAttribute("href", theme === "dark" ? "#icon-moon" : "#icon-sun");
  }

  var stored = null;
  try {
    stored = localStorage.getItem(KEY);
  } catch (e) {}
  paint(stored === "light" ? "light" : "dark");

  if (button) {
    button.addEventListener("click", function () {
      var next = root.dataset.theme === "dark" ? "light" : "dark";
      paint(next);
      try {
        localStorage.setItem(KEY, next);
      } catch (e) {}
    });
  }
})();
