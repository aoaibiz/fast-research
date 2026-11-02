// Applies the saved theme before the first paint (CSP forbids inline scripts, so this is a separate file).
(function () { var t = 'auto'; try { t = localStorage.getItem('talmudo_theme') || 'auto'; } catch (e) { /* storage blocked: follow the system */ }
  var dark = t === 'dark' || (t !== 'light' && window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches);
  document.documentElement.dataset.theme = dark ? 'dark' : 'light'; document.documentElement.dataset.themePref = t; })();
