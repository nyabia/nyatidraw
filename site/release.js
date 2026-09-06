fetch('./release.json').then(response => {
  if (!response.ok) throw new Error('Release information unavailable');
  return response.json();
}).then(release => {
  const url = new URL(release.url);
  if (url.origin !== 'https://github.com' || !url.pathname.startsWith('/nyabia/nyatidraw/releases/download/')) return;
  const download = document.getElementById('download');
  download.href = url.href;
  download.textContent = 'Windows 설치판 다운로드 ↓';
  document.getElementById('release-status').textContent = `${release.version} · Windows x64 · 알파판`;
}).catch(() => { /* Keep the ordinary Releases link usable without JavaScript/data. */ });
