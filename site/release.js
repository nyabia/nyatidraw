(() => {
  const download = document.getElementById("download");
  const label = document.getElementById("download-label");
  const status = document.getElementById("release-status");
  if (!download || !label || !status) return;

  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 7000);
  fetch("./release.json", {
    signal: abort.signal,
    credentials: "omit",
    cache: "no-cache",
  })
    .then((response) => {
      if (!response.ok) throw new Error("Release information unavailable");
      return response.json();
    })
    .then((release) => {
      if (
        !release ||
        typeof release.version !== "string" ||
        typeof release.url !== "string"
      )
        return;
      const version = release.version;
      if (
        version.length > 64 ||
        !/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$/.test(version)
      )
        return;
      const url = new URL(release.url);
      const prefix = `/nyabia/nyatidraw/releases/download/${encodeURIComponent(version)}/`;
      const installers = [
        "NyatiDraw-win-Setup.exe",
        "NyatiDraw-Alpha-win-Setup.exe",
      ];
      if (
        url.origin !== "https://github.com" ||
        url.username ||
        url.password ||
        url.search ||
        url.hash ||
        !installers.some((name) => url.pathname === prefix + name)
      )
        return;
      download.href = url.href;
      label.textContent = "Windows 설치판 다운로드";
      status.textContent = `${version} · Windows x64 · 변경 내역과 주의 사항은 Releases에서 확인하세요.`;
    })
    .catch(() => {})
    .finally(() => clearTimeout(timeout));
})();
