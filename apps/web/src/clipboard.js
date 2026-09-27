export function writePngClipboard(bytes) {
  if (!navigator.clipboard?.write || typeof ClipboardItem === "undefined") {
    return Promise.reject(new Error("이 브라우저는 이미지 클립보드 복사를 지원하지 않습니다."));
  }

  try {
    const item = new ClipboardItem({
      "image/png": Promise.resolve(new Blob([bytes], { type: "image/png" })),
    });
    return navigator.clipboard.write([item]);
  } catch (error) {
    return Promise.reject(error);
  }
}
