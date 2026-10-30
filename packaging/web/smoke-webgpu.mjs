// Browser regression smoke test for the WebGPU canvas. Start a local server for dist/web,
// then Chrome with --remote-debugging-port=9224 --enable-unsafe-webgpu before running this.
// Usage: node packaging/web/smoke-webgpu.mjs [port] [url-fragment] [backend]
const port = Number(process.argv[2] || 9224);
const fragment = process.argv[3] || '127.0.0.1';
const backend = process.argv[4] || 'BrowserWebGpu';
const pages = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = pages.find((item) => item.type === 'page' && item.url.includes(fragment));
if (!page) throw new Error(`No browser tab contains ${fragment}`);

const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.onopen = resolve;
  socket.onerror = reject;
});

let nextId = 0;
const pending = new Map();
const messages = [];
socket.onmessage = (event) => {
  const data = JSON.parse(event.data);
  if (data.id && pending.has(data.id)) {
    pending.get(data.id)(data);
    pending.delete(data.id);
    return;
  }
  if (data.method === 'Runtime.consoleAPICalled') {
    messages.push(data.params.args.map((arg) => arg.value ?? arg.description ?? '').join(' '));
  } else if (data.method === 'Runtime.exceptionThrown') {
    messages.push(`Uncaught exception: ${data.params.exceptionDetails.text}`);
  }
};
function command(method, params = {}) {
  const id = ++nextId;
  return new Promise((resolve) => {
    pending.set(id, resolve);
    socket.send(JSON.stringify({ id, method, params }));
  });
}

async function screenshotPixel(x, y) {
  const shot = await command('Page.captureScreenshot', { format: 'png' });
  const data = shot.result?.data;
  if (!data) throw new Error('Chrome did not return a canvas screenshot');
  const expression = `(async () => {
    const bytes = Uint8Array.from(atob(${JSON.stringify(data)}), c => c.charCodeAt(0));
    const bitmap = await createImageBitmap(new Blob([bytes], { type: 'image/png' }));
    const canvas = document.createElement('canvas');
    canvas.width = bitmap.width;
    canvas.height = bitmap.height;
    const context = canvas.getContext('2d');
    context.drawImage(bitmap, 0, 0);
    return [...context.getImageData(${x}, ${y}, 1, 1).data];
  })()`;
  const result = await command('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  return result.result?.result?.value;
}

try {
  await command('Runtime.enable');
  await command('Page.enable');
  await command('Page.reload', { ignoreCache: true });
  const deadline = Date.now() + 30_000;
  let state;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 250));
    const result = await command('Runtime.evaluate', {
      expression: `JSON.stringify({ loading: !!document.querySelector('#figcraft_loading'),
        width: document.querySelector('#figcraft_canvas')?.width ?? 0,
        height: document.querySelector('#figcraft_canvas')?.height ?? 0 })`,
      returnByValue: true,
    });
    state = JSON.parse(result.result?.result?.value || '{}');
    if (messages.some((message) => /panicked|Uncaught exception|browser WebGPU canvas failed/i.test(message))) {
      throw new Error(messages.filter((message) => /panicked|Uncaught exception|browser WebGPU canvas failed/i.test(message)).join('\n'));
    }
    if (messages.some((message) => message.includes(`wgpu backend ${backend}`)) && !state.loading && state.width > 0 && state.height > 0) break;
  }
  if (state?.loading || !state?.width || !state?.height || !messages.some((message) => message.includes(`wgpu backend ${backend}`))) {
    throw new Error(`${backend} canvas did not start: ${JSON.stringify(state)}; ${messages.join('\n')}`);
  }
  await new Promise((resolve) => setTimeout(resolve, 2_000));
  if (messages.some((message) => /panicked|Uncaught exception|browser WebGPU canvas failed/i.test(message))) {
    throw new Error(messages.filter((message) => /panicked|Uncaught exception|browser WebGPU canvas failed/i.test(message)).join('\n'));
  }
  // Draw in the middle of a new document, then inspect the presented pixels. Startup alone
  // does not prove that Vello's output reached eframe's browser swap chain.
  const center = { x: Math.min(560, Math.floor(state.width * 0.55)), y: Math.min(340, Math.floor(state.height * 0.45)) };
  const before = await screenshotPixel(center.x, center.y);
  await command('Input.dispatchKeyEvent', { type: 'keyDown', key: 'r', code: 'KeyR', windowsVirtualKeyCode: 82 });
  await command('Input.dispatchKeyEvent', { type: 'keyUp', key: 'r', code: 'KeyR', windowsVirtualKeyCode: 82 });
  await command('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', buttons: 1, clickCount: 1, x: center.x - 45, y: center.y - 35 });
  await command('Input.dispatchMouseEvent', { type: 'mouseMoved', button: 'left', buttons: 1, x: center.x + 45, y: center.y + 35 });
  await command('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', buttons: 0, clickCount: 1, x: center.x + 45, y: center.y + 35 });
  let after;
  const paintDeadline = Date.now() + 5_000;
  while (Date.now() < paintDeadline) {
    await new Promise((resolve) => setTimeout(resolve, 100));
    after = await screenshotPixel(center.x, center.y);
    if (after && before && after.slice(0, 3).some((channel, i) => channel > before[i] + 60)) break;
  }
  if (!after || !before || !after.slice(0, 3).some((channel, i) => channel > before[i] + 60)) {
    throw new Error(`${backend} did not present the new rectangle: before ${before}, after ${after}; ${messages.join('\n')}`);
  }
  console.log(`${backend} canvas started at ${state.width} × ${state.height} and presented a rectangle (${before} → ${after})`);
} finally {
  socket.close();
}
