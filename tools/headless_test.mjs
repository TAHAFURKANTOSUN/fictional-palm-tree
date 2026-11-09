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
//
// Çıkış kodu: tüm denetimler geçtiyse 0; bir denetim başarısızsa ya da konsola
// hata düştüyse 1 (ayrıntılar çıktıdaki `problems` listesindedir).

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

  // Beklenti tutmazsa `problems` listesine yazar; test sonunda çıkış kodu
  // buna göre belirlenir.
  const expect = (name, ok, detail) => {
    if (!ok) problems.push(`BEKLENTİ: ${name}` + (detail === undefined ? '' : ` — ${JSON.stringify(detail)}`));
  };
  const state = async () => JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
  // Chunk akışı yetişene kadar bekler (yavaş makinelerde sabit bekleme yetmeyebilir).
  const waitForStreaming = async (ms = 20000) => {
    let s = await state();
    for (let waited = 0; s.meshPending > 0 && waited < ms; waited += 250) {
      await sleep(250);
      s = await state();
    }
    return s;
  };

  // Görüş mesafesindeki her chunk mesh'lenmiş olmalı. (Eskiden dairenin
  // kenarındaki chunk'ların çapraz komşuları üretilmediği için bunlar hiç
  // çizilemiyordu.)
  expect('açılışta görüş mesafesindeki tüm chunk\'lar mesh\'lendi', boot.meshPending === 0,
    { wantMesh: boot.wantMesh, meshPending: boot.meshPending });

  // ---- 1) Oyna düğmesine bas (başlat) ----
  await evalJs(`document.getElementById('startBtn').click()`);
  await sleep(600);
  out.afterStart = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));

  // ---- 2) Başlangıç kiti ----
  out.starterGrass = await evalJs(`window.__vcRs.debug_count(1)`);
  out.starterApple = await evalJs(`window.__vcRs.debug_count(101)`);
  expect('başlangıç kiti: 16 çim, 3 elma', out.starterGrass === 16 && out.starterApple === 3,
    [out.starterGrass, out.starterApple]);

  // ---- 3) Hasar ----
  await evalJs(`window.__vcRs.debug_set_health(20); window.__vcRs.debug_damage(6); 1`);
  out.afterDamage = JSON.parse(await evalJs(`window.__vcRs.debug_state()`)).health;
  expect('6 hasar: can 20 → 14', out.afterDamage === 14, out.afterDamage);

  // ---- 4) Yıldız açlığı: 20 sn simülasyon ----
  await evalJs(`window.__vcRs.debug_set_health(20); window.__vcRs.debug_set_hunger(0); window.__vcRs.debug_sim(20); 1`);
  out.starvedHealth = JSON.parse(await evalJs(`window.__vcRs.debug_state()`)).health;
  expect('20 sn yıldız açlığı: can 20 → 15', out.starvedHealth === 15, out.starvedHealth);

  // ---- 5) Tokken iyileşme ----
  await evalJs(`window.__vcRs.debug_set_health(8); window.__vcRs.debug_set_hunger(20); window.__vcRs.debug_sim(20); 1`);
  out.regenHealth = JSON.parse(await evalJs(`window.__vcRs.debug_state()`)).health;
  expect('tokken iyileşme: can 8\'den artar, 12\'yi geçmez', out.regenHealth > 8 && out.regenHealth <= 12,
    out.regenHealth);

  // ---- 6) Yaratıcı mod: hasar yok, 13 yuva x64, uçuş açık ----
  await evalJs(`window.__vcRs.debug_set_health(20); window.__vcRs.debug_set_mode(true); 1`);
  await sleep(200);
  let st = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
  out.creative = { mode: st.mode, flying: st.flying, slots: st.slots.filter(s => s).length, first: st.slots[0] };
  await evalJs(`window.__vcRs.debug_damage(10); 1`);
  out.creativeDamageIgnored = JSON.parse(await evalJs(`window.__vcRs.debug_state()`)).health === 20;
  expect('yaratıcı mod: uçuş açık, 13 yuva × 64',
    out.creative.mode === 'Yaratıcı' && out.creative.flying === true && out.creative.slots === 13 &&
      out.creative.first?.[0] === 1 && out.creative.first?.[1] === 64, out.creative);
  expect('yaratıcı modda hasar yok', out.creativeDamageIgnored);

  // ---- 7) Hayatta kalmaya dönüş: envanter geri gelir ----
  await evalJs(`window.__vcRs.debug_set_mode(false); 1`);
  await sleep(200);
  st = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
  out.backToSurvival = { mode: st.mode, flying: st.flying, grass: st.slots[0] };
  expect('hayatta kalmaya dönüş: uçuş kapalı, envanter geri geldi',
    st.mode === 'Hayatta Kalma' && st.flying === false && st.slots[0]?.[0] === 1 && st.slots[0]?.[1] === 16,
    out.backToSurvival);

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
  expect('yaratıcı modda koymak blok harcamaz', out.creativePlaceNoConsume);

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
  expect('elma: açlık 6 → 12, bir elma eksilir',
    afterEat.hunger === 12 && appleBefore?.[0] === 101 && afterEat.slots[8]?.[1] === appleBefore[1] - 1, out.eat);

  // ---- 11) Uçuş hayatta kalmada engelli ----
  out.flyBlocked = afterEat.flying === false;
  expect('hayatta kalmada uçuş kapalı', out.flyBlocked);

  // ---- 12) FPS + çizim (yatay bakış) ----
  await evalJs(`window.__vcRs.debug_set_look(0.9, -0.05); 1`);
  await sleep(2500);
  st = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
  out.perf = { fps: st.fps, chunks: st.chunks, draws: st.draws, tris: st.tris, pos: st.pos };

  // ---- 13) Uzak ışınlanma (sonsuz dünya akışı) ----
  // İkinci nokta, chunk uzaklığının karesi 32 biti taşacak kadar uzaktır:
  // eski chunk'lar yine de boşaltılmalı (bellek sınırı korunmalı).
  //
  // Yaratıcı modda yapılır: hayatta kalma modunda y=90'a ışınlanan oyuncu
  // düşüp ölür, yeniden doğuş da sonraki adımların (yüzme testi) ışınlamasını
  // zamanlamaya bağlı olarak ezer.
  const home = (await state()).pos;
  await evalJs(`window.__vcRs.debug_set_mode(true); 1`);
  out.farTeleport = [];
  for (const [x, z] of [[50000.5, -50000.5], [1000000.5, -1000000.5]]) {
    await evalJs(`window.__vcRs.debug_teleport(${x}, 90, ${z}); 1`);
    await sleep(500);
    st = await waitForStreaming();
    const r = { chunks: st.chunks, gpuChunks: st.gpuChunks, wantMesh: st.wantMesh,
      meshPending: st.meshPending, pos: st.pos, fps: st.fps };
    out.farTeleport.push(r);
    expect(`ışınlanma (${x}, ${z}): akış tamamlandı`, st.meshPending === 0, r);
    expect(`ışınlanma (${x}, ${z}): eski chunk'lar boşaltıldı`,
      st.chunks <= boot.chunks * 1.5 && st.gpuChunks <= st.chunks, r);
  }
  // Başlangıç noktasına, zemine geri dön ve hayatta kalmaya geç.
  await evalJs(`window.__vcRs.debug_teleport(${home[0]}, ${home[1] + 0.05}, ${home[2]}); 1`);
  await sleep(500);
  await waitForStreaming();
  await evalJs(`window.__vcRs.debug_set_mode(false); 1`);
  await sleep(300);

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

  // ---- 16) H2: pencere yeniden boyutlanınca HUD ve en-boy oranı güncelleniyor mu? ----
  {
    const before = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
    await send('Emulation.setDeviceMetricsOverride', {
      width: 900, height: 520, deviceScaleFactor: 1, mobile: false,
    });
    // Boyut değişimini deterministik tetikle (tarayıcı da aynı olayı gönderir).
    await evalJs(`window.dispatchEvent(new Event('resize')); 1`);
    await sleep(600);
    const inner = await evalJs(`[window.innerWidth, window.innerHeight]`);
    const after = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
    await send('Emulation.clearDeviceMetricsOverride');
    await evalJs(`window.dispatchEvent(new Event('resize')); 1`);
    await sleep(300);

    out.resize = {
      inner,
      hud: [after.hudW, after.hudH],
      before: [before.hudW, before.hudH],
      ok: after.hudW === inner[0] && after.hudH === inner[1],
    };
    if (!out.resize.ok) {
      problems.push(
        `H2: HUD boyutu pencereye uymuyor — inner=${inner}, hud=[${after.hudW},${after.hudH}]`
      );
    }
  }

  // ---- 17) H6: suda yüzme (Space ile yüzeye çıkabilme) ----
  {
    await evalJs(`window.__vcRs.debug_set_mode(false); window.__vcRs.debug_set_health(20); 1`);
    await sleep(1900);
    const found = await evalJs(`window.__vcRs.debug_teleport_to_water()`);
    await sleep(1800); // chunk akışı + yerleşme
    const inWater = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
    const yBefore = inWater.pos[1];

    // Space tuşunu basılı tut (gerçek klavye olayı).
    await evalJs(
      `window.dispatchEvent(new KeyboardEvent('keydown', { code: 'Space', bubbles: true })); 1`
    );
    await sleep(1500);
    const afterUp = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
    await evalJs(
      `window.dispatchEvent(new KeyboardEvent('keyup', { code: 'Space', bubbles: true })); 1`
    );
    await sleep(400);

    out.swim = {
      found,
      feetInWater: inWater.feetInWater,
      yBefore,
      yAfter: afterUp.pos[1],
      rose: afterUp.pos[1] > yBefore + 0.5,
    };
    if (!found) {
      problems.push('H6: okyanus sütunu bulunamadı');
    } else if (!inWater.feetInWater) {
      problems.push(`H6: ışınlanılan noktada ayaklar suda değil (y=${yBefore})`);
    } else if (!out.swim.rose) {
      problems.push(
        `H6: suda Space ile yükselme yok — y ${yBefore} → ${afterUp.pos[1]}`
      );
    }

    // Su bırakılınca batma hızı sınırlı olmalı (eskiden -70 m/s'ye kadar düşüyordu).
    await sleep(900);
    const sink = JSON.parse(await evalJs(`window.__vcRs.debug_state()`));
    out.swim.sinkDrop = Number((afterUp.pos[1] - sink.pos[1]).toFixed(2));
    if (out.swim.sinkDrop > 4.0) {
      problems.push(`H6: suda batma çok hızlı (0,9 sn'de ${out.swim.sinkDrop} blok)`);
    }
  }

  out.problems = problems;
  console.log(JSON.stringify(out, null, 2));
  if (problems.length > 0) {
    console.log(`BAŞARISIZ: ${problems.length} sorun`);
    process.exit(1);
  }
  console.log('BAŞARILI: tüm denetimler geçti');
  process.exit(0);
}

main().catch(e => { console.log('ERR', e.message); process.exit(1); });
