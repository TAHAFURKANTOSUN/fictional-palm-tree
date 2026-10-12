// VoxelCraft (Rust/WASM) — başsız (headless) tarayıcı duman testi.
//
// Oyunu gerçek bir Chrome'da açar, `window.__vcRs` hata ayıklama kancası
// üzerinden oyun kurallarını sürer ve konsol/Promise hatalarını toplar.
//
// Kullanım (ÖNERİLEN): kurulum + test + temizliği tek adımda ve GÜVENLİ yapar
//   ./build.sh
//   ./tools/smoke.sh                 # varsayılan 8799 portu
//   ./tools/smoke.sh 8811            # özel port
//
// Elle çalıştırmak isterseniz:
//   python -m http.server 8799 --bind 127.0.0.1 &   # file:// CORS'a takılır
//   chrome --headless=new --remote-debugging-port=9334 \
//          --enable-unsafe-swiftshader \
//          --user-data-dir=<izole dizin> \
//          http://127.0.0.1:8799/index.html &
//   node tools/headless_test.mjs
//
// !! TEMİZLİK UYARISI !!
//   Chrome'u `taskkill /F /IM chrome.exe` ile KAPATMAYIN. `/IM` görüntü adına
//   göre eşleşir ve kullanıcının açık Chrome pencerelerini de zorla kapatır.
//   Yalnızca kendi başlattığınız süreci kapatın:
//     taskkill /F /T /PID <CDP portunu dinleyen PID>
//
// Ortam değişkenleriyle özelleştirilebilir:
//   CDP_PORT (varsayılan 9334), TARGET_URL (varsayılan http://127.0.0.1:8799/index.html)

import fs from 'fs';

const PORT = Number(process.env.CDP_PORT || 9334);
const TARGET = process.env.TARGET_URL || 'http://127.0.0.1:8799/index.html';
// http(s) için port, file:// için dosya adı ile eşleştiriyoruz.
const TARGET_MATCH = (() => {
  const u = new globalThis.URL(TARGET);
  return u.port || u.pathname.split('/').pop() || '';
})();
const sleep = ms => new Promise(r => setTimeout(r, ms));

async function main() {
  let page = null;
  for (let i = 0; i < 80; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json();
      page = list.find(t => t.type === 'page' && t.url.includes(TARGET_MATCH));
      if (page) break;
    } catch (e) { /* bekliyoruz */ }
    await sleep(400);
  }
  if (!page) { console.log('NO TARGET'); process.exit(1); }

  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });

  let id = 0;
  const pending = new Map();
  const problems = [];

  ws.onmessage = ev => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); return; }
    if (m.method === 'Runtime.exceptionThrown') {
      const d = m.params.exceptionDetails;
      problems.push('EXCEPTION: ' + (d.exception?.description || d.text));
    }
    if (m.method === 'Log.entryAdded' && m.params.entry.level === 'error') {
      problems.push('LOG ERROR: ' + m.params.entry.text);
    }
    if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') {
      problems.push('CONSOLE ERROR: ' + m.params.args.map(a => a.value ?? a.description).join(' '));
    }
  };

  const send = (method, params) => new Promise(res => {
    const i = ++id; pending.set(i, res);
    ws.send(JSON.stringify({ id: i, method, params }));
  });

  await send('Runtime.enable');
  await send('Log.enable');
  await send('Page.enable');

  const evalJs = async (expr, awaitPromise = false) => {
    const r = await send('Runtime.evaluate', { expression: expr, awaitPromise, returnByValue: true });
    if (r.result?.exceptionDetails) {
      return { __error: r.result.exceptionDetails.exception?.description || r.result.exceptionDetails.text };
    }
    return r.result?.result?.value;
  };

  // wasm-bindgen modülünün yüklenmesini bekle
  let ready = false;
  for (let i = 0; i < 120; i++) {
    const v = await evalJs(`typeof window.__vcRs !== 'undefined'`);
    if (v === true) { ready = true; break; }
    await sleep(500);
  }
  console.log('modül yüklendi:', ready);
  if (!ready) {
    console.log('problems:', problems);
    process.exit(1);
  }

  // dünya kurulumu bitsin
  let boot = null;
  for (let i = 0; i < 120; i++) {
    boot = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
    if (boot && boot.booting === false) break;
    await sleep(500);
  }
  console.log('boot bitti:', boot && boot.booting === false);

  const out = {};
  out.boot = boot;

  // ---- 1) Oyna düğmesine bas (başlat) ----
  await evalJs(`document.getElementById('startBtn').click()`);
  await sleep(600);
  out.afterStart = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));

  // ---- 2) Başlangıç kiti ----
  out.starterGrass = await evalJs(`window.__vcRs.debug_count(1)`);
  out.starterApple = await evalJs(`window.__vcRs.debug_count(101)`);

  // ---- 3) Hasar ----
  await evalJs(`window.__vcRs.debug_set_health(20); window.__vcRs.debug_damage(6); 1`);
  out.afterDamage = JSON.parse(await evalJs(`window.__vcRs.debug_state()`)).health;

  // ---- 4) Yıldız açlığı: 20 sn simülasyon ----
  await evalJs(`window.__vcRs.debug_set_health(20); window.__vcRs.debug_set_hunger(0); window.__vcRs.debug_sim(20); 1`);
  out.starvedHealth = JSON.parse(await evalJs(`window.__vcRs.debug_state()`)).health;

  // ---- 5) Tokken iyileşme ----
  await evalJs(`window.__vcRs.debug_set_health(8); window.__vcRs.debug_set_hunger(20); window.__vcRs.debug_sim(20); 1`);
  out.regenHealth = JSON.parse(await evalJs(`window.__vcRs.debug_state()`)).health;

  // ---- 6) Yaratıcı mod: hasar yok, 13 yuva x64, uçuş açık ----
  await evalJs(`window.__vcRs.debug_set_health(20); window.__vcRs.debug_set_mode(true); 1`);
  await sleep(200);
  let st = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
  out.creative = { mode: st.mode, flying: st.flying, slots: st.slots.filter(s => s).length, first: st.slots[0] };
  await evalJs(`window.__vcRs.debug_damage(10); 1`);
  out.creativeDamageIgnored = JSON.parse(await evalJs(`window.__vcRs.debug_state()`)).health === 20;

  // ---- 7) Hayatta kalmaya dönüş: envanter geri gelir ----
  await evalJs(`window.__vcRs.debug_set_mode(false); 1`);
  await sleep(200);
  st = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
  out.backToSurvival = { mode: st.mode, flying: st.flying, grass: st.slots[0] };

  // ---- 8) Blok kır / koy (aşağı bak) ----
  await evalJs(`window.__vcRs.debug_set_look(0, -1.5); 1`);
  await sleep(300);
  await evalJs(`window.__vcRs.debug_set_mode(true); 1`);
  await sleep(200);
  await evalJs(`window.__vcRs.debug_select(0); 1`);
  const before = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
  const creativeConsumed = await evalJs(`window.__vcRs.debug_place_consumed()`);
  const afterPlace = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
  out.creativePlaceNoConsume = creativeConsumed === 0 &&
    before.slots[0][1] === afterPlace.slots[0][1];

  await evalJs(`window.__vcRs.debug_break(); 1`);
  await sleep(100);
  out.afterBreak = JSON.parse(await evalJs(`window.__vcRs.debug_state()`)).slots[0][1];

  // ---- 9) Hayatta kalmada koyma harcamalı ----
  await evalJs(`window.__vcRs.debug_set_mode(false); 1`);
  await sleep(1900);
  await evalJs(`window.__vcRs.debug_select(0); 1`);
  // Yere ~4 blok ileride bak: hedef oyuncunun gövdesine girmesin.
  await evalJs(`window.__vcRs.debug_set_look(2.4, -0.55); 1`);
  await sleep(400);
  out.survivalPlaceConsumed = await evalJs(`window.__vcRs.debug_place_consumed()`);

  // ---- 10) Yemek ----
  await evalJs(`window.__vcRs.debug_set_hunger(6); window.__vcRs.debug_add_item(101, 3); 1`);
  // elmayı kısayola koy: 8. yuva zaten elma olabilir, garantiye al
  await evalJs(`window.__vcRs.debug_select(8); 1`);
  const appleBefore = JSON.parse(await evalJs(`window.__vcRs.debug_state()`)).slots[8];
  await evalJs(`window.__vcRs.debug_right_click(); 1`);
  const afterEat = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
  out.eat = { before: appleBefore, after: afterEat.slots[8], hunger: afterEat.hunger };

  // ---- 11) Uçuş hayatta kalmada engelli ----
  out.flyBlocked = afterEat.flying === false;

  // ---- 12) FPS + çizim (yatay bakış) ----
  await evalJs(`window.__vcRs.debug_set_look(0.9, -0.05); 1`);
  await sleep(2500);
  st = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
  out.perf = { fps: st.fps, chunks: st.chunks, draws: st.draws, tris: st.tris, pos: st.pos };

  // ---- 13) Uzak ışınlanma (sonsuz dünya akışı) ----
  await evalJs(`window.__vcRs.debug_teleport(50000.5, 90, -50000.5); 1`);
  await sleep(3500);
  st = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
  out.farTeleport = { chunks: st.chunks, pos: st.pos, fps: st.fps };

  // ---- 14) Envanter ekranı ----
  await evalJs(`window.__vcRs.debug_set_open(true); 1`);
  await sleep(500);
  out.invOpen = JSON.parse(await evalJs(`window.__vcRs.debug_state()`)).started;
  let shot = await send('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync('_rs_inventory.png', Buffer.from(shot.result.data, 'base64'));
  await evalJs(`window.__vcRs.debug_set_open(false); 1`);
  await sleep(400);

  // ---- 15) Oyun içi ekran görüntüsü ----
  await evalJs(`window.__vcRs.debug_set_mode(false); window.__vcRs.debug_set_health(13); window.__vcRs.debug_set_hunger(11); window.__vcRs.debug_set_look(1.0, -0.12); 1`);
  await sleep(2500);
  shot = await send('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync('_rs_game.png', Buffer.from(shot.result.data, 'base64'));

  out.problems = problems;
  console.log(JSON.stringify(out, null, 2));
  process.exit(0);
}

main().catch(e => { console.log('ERR', e.message); process.exit(1); });
