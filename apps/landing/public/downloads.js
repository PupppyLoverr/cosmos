(() => {
  // Cosmos release artifacts live in the public cosmos-releases repo. Only
  // macOS ships a file today; other platforms link to the artifacts folder
  // until their packages are published.
  const base = "https://github.com/PupppyLoverr/cosmos-releases/raw/main/artifacts/v0.2.1-alpha/";
  const folder = "https://github.com/PupppyLoverr/cosmos-releases/tree/main/artifacts/v0.2.1-alpha";
  const releases = {
    macos: { file: "macos-arm64.dmg", label: "Download for macOS", detail: "Apple silicon" },
    windows: { label: "Download for Windows", detail: "Windows x64 · Portable ZIP" },
    linux: { label: "Download for Linux", detail: "Linux x64" },
    "linux-arm": { label: "Download for Linux", detail: "Linux ARM64" },
  };
  const urlFor = (version, key) => {
    const release = releases[key];
    return release && release.file ? `${base}cosmos-${version}-${release.file}` : folder;
  };
  const ua = navigator.userAgent || "";
  const platform = navigator.userAgentData?.platform || navigator.platform || "";
  const mobile = /Android|iPhone|iPad|iPod/i.test(ua) || navigator.userAgentData?.mobile
    || (/Mac/i.test(platform) && navigator.maxTouchPoints > 1);
  const os = mobile ? null : /Win/i.test(platform) ? "windows"
    : /Mac/i.test(platform) ? "macos" : /Linux/i.test(platform)
    ? (/aarch64|arm64/i.test(`${platform} ${ua}`) ? "linux-arm" : "linux") : null;
  const apply = (version) => {
    for (const link of document.querySelectorAll("[data-platform-download]")) {
      link.href = urlFor(version, link.dataset.platformDownload);
    }
    for (const id of ["nav-download", "hero-download", "closing-download"]) {
      const link = document.getElementById(id);
      if (!link || !os) continue;
      const { label, detail } = releases[os];
      link.href = urlFor(version, os);
      link.textContent = id === "nav-download" ? "Download" : label;
      link.setAttribute("aria-label", `${label} (${detail})`);
      link.title = detail;
    }
    document.getElementById("ver").textContent = `v${version}`;
  };
  // A published fallback keeps downloads usable without the version endpoint.
  const fallback = "0.2.79";
  apply(fallback);
  // No latest.txt on the releases repo — keep the fallback until one exists.
  fetch(`${base}latest.txt`, { credentials: "omit" })
    .then((r) => r.ok ? r.text() : Promise.reject())
    .then((text) => {
      const version = text.trim();
      if (!/^\d+\.\d+\.\d+$/.test(version)) return;
      const a = version.split(".").map(Number), b = fallback.split(".").map(Number);
      const first = a.findIndex((value, i) => value !== b[i]);
      if (first !== -1 && a[first] < b[first]) return;
      apply(version);
    }).catch(() => {});
})();
