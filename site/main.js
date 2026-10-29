// silent's homepage: the theme toggle, clips that play while they are seen,
// the copy buttons, and the benchmark chart drawn from its table.

(function () {
  "use strict";
  var root = document.documentElement;
  var system = window.matchMedia("(prefers-color-scheme: dark)");

  // Light or dark. A choice that matches the system's goes back to following
  // the system, so nothing is stored for it.
  function systemTheme() { return system.matches ? "dark" : "light"; }
  function shownTheme() { return root.dataset.theme || systemTheme(); }
  var toggle = document.querySelector(".theme-toggle");
  function describe() {
    if (!toggle) return;
    var theme = shownTheme();
    var following = !root.dataset.theme;
    var other = theme === "dark" ? "light" : "dark";
    toggle.setAttribute("aria-pressed", theme === "dark" ? "true" : "false");
    toggle.setAttribute("aria-label", "Dark theme");
    toggle.title = (theme === "dark" ? "Dark" : "Light") + " theme" +
      (following ? ", as the system is" : ", chosen here") + ". Click for " + other + ".";
  }
  if (toggle) {
    toggle.addEventListener("click", function () {
      var next = shownTheme() === "dark" ? "light" : "dark";
      try {
        if (next === systemTheme()) {
          delete root.dataset.theme;
          localStorage.removeItem("silent-theme");
        } else {
          root.dataset.theme = next;
          localStorage.setItem("silent-theme", next);
        }
      } catch (e) {
        root.dataset.theme = next;
      }
      describe();
    });
  }
  if (system.addEventListener) system.addEventListener("change", describe);
  describe();

  // Each clip's poster loads as it comes near; the variant of the other
  // theme is hidden, never comes near, and so loads nothing.
  var still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var videos = Array.prototype.slice.call(document.querySelectorAll("video.clip"));
  function poster(v) { if (v.dataset.poster && !v.poster) v.poster = v.dataset.poster; }
  if ("IntersectionObserver" in window) {
    var near = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (entry.isIntersecting) { poster(entry.target); near.unobserve(entry.target); }
      });
    }, { rootMargin: "800px 0px" });
    videos.forEach(function (v) { near.observe(v); });
  } else {
    videos.forEach(poster);
  }

  // Clips play while they are on screen. With motion to be reduced they
  // never start by themselves: they show their poster and their controls.
  if (still) {
    videos.forEach(function (v) { v.controls = true; });
  } else if ("IntersectionObserver" in window) {
    var seen = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        var v = entry.target;
        if (entry.isIntersecting) {
          if (v.preload === "none") v.preload = "auto";
          var playing = v.play();
          if (playing && playing.catch) playing.catch(function () { v.controls = true; });
        } else {
          v.pause();
        }
      });
    }, { threshold: 0.35 });
    videos.forEach(function (v) { seen.observe(v); });
  } else {
    videos.forEach(function (v) { v.controls = true; });
  }

  // Copy buttons: each copies the commands of the block in its card.
  document.querySelectorAll(".copy").forEach(function (button) {
    button.addEventListener("click", function () {
      var card = button.closest(".install-item");
      var code = card && card.querySelector("pre code");
      if (!code || !navigator.clipboard) return;
      navigator.clipboard.writeText(code.textContent).then(function () {
        button.textContent = "Copied";
        setTimeout(function () { button.textContent = "Copy"; }, 1600);
      }, function () {});
    });
  });

  // The benchmark: two columns of bars, warm and cold, drawn from the table,
  // which stays the source.
  var chart = document.getElementById("chart");
  if (!chart) return;
  var grid = chart.querySelector(".chart-grid");
  var rows = Array.prototype.slice.call(chart.querySelectorAll("tbody tr")).map(function (tr) {
    var cells = tr.querySelectorAll("td");
    return {
      label: tr.querySelector("th").textContent,
      warm: [cells[0].textContent.trim(), cells[1].textContent.trim()],
      cold: [cells[2].textContent.trim(), cells[3].textContent.trim()],
    };
  });
  var modes = [
    { key: "warm", title: "Already running (warm)" },
    { key: "cold", title: "Started for the file (cold)" },
  ];
  // One scale for both columns, so that bars compare across them too.
  var most = 0;
  rows.forEach(function (r) {
    r.warm.concat(r.cold).forEach(function (text) { var ms = Number(text); if (!isNaN(ms) && ms > most) most = ms; });
  });
  var steps = [5, 10, 20, 25, 50, 100, 200, 250, 500, 1000];
  var step = steps.filter(function (s) { return most / s <= 4; })[0] || 1000;
  modes.forEach(function (mode) {
    mode.max = Math.ceil(most / step) * step;
    mode.step = step;
  });

  function element(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  }
  grid.appendChild(element("div", "chart-head chart-corner", ""));
  modes.forEach(function (mode) { grid.appendChild(element("div", "chart-head", mode.title)); });
  rows.forEach(function (r) {
    grid.appendChild(element("div", "chart-label", r.label));
    modes.forEach(function (mode) {
      var cell = element("div", "chart-cell");
      var cannot = false;
      ["silent", "sushi"].forEach(function (who, i) {
        var text = r[mode.key][i];
        var ms = Number(text);
        if (isNaN(ms)) {
          if (cannot) return;
          cannot = text === "needs LibreOffice";
          cell.appendChild(element("div", "bar-missing", cannot ? "sushi needs LibreOffice" : who + ": " + text));
          return;
        }
        var bar = element("div", "bar " + who);
        bar.style.width = (100 * ms / mode.max) + "%";
        bar.title = r.label + ", " + who + ", " + mode.key + ": " + ms + " ms";
        bar.appendChild(element("span", "bar-value", String(ms)));
        cell.appendChild(bar);
      });
      grid.appendChild(cell);
    });
  });
  grid.appendChild(element("div", "chart-corner", ""));
  modes.forEach(function (mode) {
    var axis = element("div", "chart-axis");
    for (var v = 0; v <= mode.max; v += mode.step) {
      var tick = element("span", "", String(v));
      tick.style.left = (100 * v / mode.max) + "%";
      if (v === mode.max) tick.style.transform = "translateX(-100%)";
      axis.appendChild(tick);
    }
    grid.appendChild(axis);
  });
})();
