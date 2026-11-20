// Self-test: starts the fixture server and drives every task through the real
// page and submission path with the frob CLI, checking that /__state (or the
// answer pattern) reports failure for a wrong attempt and success for a right one.
//
// Usage: node selftest.mjs [--port 4518] [--only id1,id2]
// Env: FROB_BIN overrides the frob binary.

import { execFile } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { startServer, TASK_IDS } from './server.mjs';

const FROB = process.env.FROB_BIN || '/Users/hareshk/dev/frob/cli/target/release/frob';
const SESSION = 'fxtest';
const argv = process.argv.slice(2);
const opt = (k, d) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : d; };
const PORT = +opt('--port', 4518);
const ONLY = opt('--only', '')?.split(',').filter(Boolean);
const BASE = `http://127.0.0.1:${PORT}`;
const tasks = JSON.parse(readFileSync(new URL('./tasks.json', import.meta.url), 'utf8'));

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// Async exec: the server runs in this process, so a blocking exec would starve it.
function run(args, input) {
  return new Promise((resolve, reject) => {
    const cp = execFile(FROB, ['--session', SESSION, ...args], { encoding: 'utf8', timeout: 60000, maxBuffer: 32 << 20 }, (err, stdout, stderr) => (err && !stdout ? reject(new Error(stderr || err.message)) : resolve(stdout)));
    if (input !== undefined) { cp.stdin.end(input); }
  });
}
const frob = (...args) => run(args);

const HELPERS = `
const sleep=(ms)=>new Promise(r=>setTimeout(r,ms));
const $=(s,r=document)=>r.querySelector(s);
const $$=(s,r=document)=>[...r.querySelectorAll(s)];
const waitFor=async(fn,t=6000)=>{const end=Date.now()+t;while(Date.now()<end){let v;try{v=fn()}catch{}if(v)return v;await sleep(50)}throw new Error('waitFor timeout: '+fn)};
const setVal=(el,v)=>{el.focus();el.value=v;el.dispatchEvent(new Event('input',{bubbles:true}));el.dispatchEvent(new Event('change',{bubbles:true}));};
const byText=(sel,txt,r=document)=>$$(sel,r).find(e=>e.textContent.trim()===txt);
const later=(fn)=>{setTimeout(fn,50);return 'scheduled'};
`;

async function js(code) {
  const out = await run(['--json', 'eval', '--stdin'], `(async()=>{${HELPERS}\n${code}\n})()`);
  const r = JSON.parse(out);
  if (!r.success) throw new Error(r.error);
  return r.data.result;
}

async function state(id) { return (await fetch(`${BASE}/__state/${id}`)).json(); }
async function reset(id) { await fetch(`${BASE}/__reset/${id}`, { method: 'POST' }); }
async function pollState(id, want, ms = 6000) {
  const end = Date.now() + ms; let s;
  do { s = await state(id); if (s.pass === want) return s; await sleep(150); } while (Date.now() < end);
  return s;
}

// Each entry: wrong/right are lists of in-page JS steps run in order on a freshly
// opened start page. Steps may end by scheduling a navigation via later(...).
// Answer tasks give `extract` (JS returning the correct reply) and `wrongReplies`.
const S = {
  'login-validation': {
    wrong: [`setVal($('#email'),'maria.chen@acme.test');setVal($('#password'),'harbor-421');$('button[type=submit]').click();await waitFor(()=>$('#err').textContent);return $('#err').textContent;`],
    right: [`setVal($('#email'),'maria.chen@acme.test');setVal($('#password'),'harbor-42');$('button[type=submit]').click();
await waitFor(()=>!$('#termsWrap').hidden); const msg=$('#err').textContent; $('#terms').click(); later(()=>$('button[type=submit]').click()); return msg;`,
      `await waitFor(()=>document.body.innerText.includes('Welcome back, Maria')); return location.pathname;`],
  },
  'checkout-wizard': (() => {
    const fill = (ship) => `setVal($('#name'),'Jonas Weber');setVal($('#street'),'742 Evergreen Terrace');setVal($('#city'),'Springfield');setVal($('#state'),'OR');setVal($('#zip'),'97477');
$('input[name=ship][value=${ship}]').click();$('#next1').click();if($('#step2').hidden) throw new Error('step1: '+$('#e1').textContent);
setVal($('#card'),'4242 4242 4242 4242');setVal($('#exp'),'08/29');setVal($('#cvc'),'314');$('#next2').click();if($('#step3').hidden) throw new Error('step2: '+$('#e2').textContent);
$('#place').click(); await sleep(100); if(!$('#e3').textContent) throw new Error('agree checkbox not enforced');
$('#agree').click();$('#place').click();await waitFor(()=>!$('#done').hidden);return $('#doneMsg').textContent;`;
    return { wrong: [fill('standard')], right: [fill('express')] };
  })(),
  'cookie-consent': {
    wrong: [`byText('button','Accept all').click();await sleep(200);byText('button','Choose Team').click();await waitFor(()=>!$('#result').hidden);return $('#result').textContent;`],
    right: [`byText('button','Choose Team').click();await sleep(300);if(!$('#result').hidden) throw new Error('overlay did not block');
byText('button','Reject non-essential').click();await waitFor(()=>!$('#consent'));byText('button','Annual (save 20%)').click();byText('button','Choose Team').click();await waitFor(()=>!$('#result').hidden);return $('#result').textContent;`],
  },
  'newsletter-popup': (() => {
    const fill = `setVal($('#subject'),'Invoice shows wrong VAT number');setVal($('#category'),'billing');$('input[name=priority][value=high]').click();setVal($('#message'),'Our March invoice lists the wrong VAT number. Please correct the VAT number to DE298374651.');`;
    return {
      wrong: [`await waitFor(()=>$('#promo'),4000);setVal($('#nemail'),'me@example.com');$('#nf button[type=submit]').click();await waitFor(()=>!$('#promo'));${fill}$('#f button[type=submit]').click();await waitFor(()=>!$('#sent').hidden);return 'sent';`],
      right: [`${fill}await waitFor(()=>$('#promo'),4000);$('#f button[type=submit]').click();await sleep(300);if(!$('#sent').hidden) throw new Error('popup did not block');
$('#promo button[aria-label=Close]').click();await waitFor(()=>!$('#promo'));$('#f button[type=submit]').click();await waitFor(()=>!$('#sent').hidden);return $('#sent').textContent;`],
    };
  })(),
  combobox: {
    wrong: [`const i=$('#country');setVal(i,'New Zealand');i.blur();await sleep(300);$('#save').click();await sleep(200);if(!$('#e').textContent) throw new Error('free text accepted');
setVal(i,'new c');(await waitFor(()=>byText('[role=option]','New Caledonia'))).click();$('#save').click();await sleep(300);return i.value;`],
    right: [`const i=$('#country');setVal(i,'new z');const o=await waitFor(()=>byText('[role=option]','New Zealand'));o.click();if(i.value!=='New Zealand') throw new Error('not chosen');$('#save').click();await sleep(300);return i.value;`],
  },
  datepicker: (() => {
    const go = (label, t) => `setVal($('#nm'),'Ava Patel');setVal($('#em'),'ava.patel@example.com');$('#pick').click();await waitFor(()=>!$('#cal').hidden);
$('#next').click();$('#next').click();const b=await waitFor(()=>$('button[aria-label="${label}"]'));b.click();
const s=await waitFor(()=>$('[data-t="${t}"]'));s.click();$('#book').click();await waitFor(()=>!$('#done').hidden||$('#e').textContent);return $('#date').value+' '+$('#done').textContent+$('#e').textContent;`;
    return { wrong: [go('Wednesday, May 13, 2026', '15:30')], right: [go('Thursday, May 14, 2026', '15:30')] };
  })(),
  'iframe-form': {
    wrong: [`setVal($('#pemail'),'ap@northwind.example');$('#saveProfile').click();await sleep(300);return 'profile saved';`],
    right: [`const d=await waitFor(()=>$('iframe').contentDocument?.querySelector('#vat'));const f=$('iframe').contentDocument;setVal(f.querySelector('#bemail'),'ap@northwind.example');setVal(f.querySelector('#vat'),'GB123456789');
f.querySelector('button[type=submit]').click();await waitFor(()=>f.querySelector('#msg').textContent);return f.querySelector('#msg').textContent;`],
  },
  'shadow-dom': {
    wrong: [`setVal($('#promo'),'GIFT-7Q2M-K9XD');$('#applyPromo').click();await waitFor(()=>$('#promoMsg').textContent);
const r=$('gift-card-redeemer').shadowRoot;setVal(r.querySelector('ui-text-field').shadowRoot.querySelector('input'),'GIFT-7Q2M-K9DX');r.querySelector('ui-button').shadowRoot.querySelector('button').click();await waitFor(()=>r.querySelector('.msg').textContent);return r.querySelector('.msg').textContent;`],
    right: [`const r=$('gift-card-redeemer').shadowRoot;setVal(r.querySelector('ui-text-field').shadowRoot.querySelector('input'),'GIFT-7Q2M-K9XD');r.querySelector('ui-button').shadowRoot.querySelector('button').click();await waitFor(()=>r.querySelector('.msg').textContent);return r.querySelector('.msg').textContent;`],
  },
  'table-pagination': {
    extract: `const hb=()=>$('#hdr button[data-k=salary]');hb().click();hb().click();if($('#hdr th[aria-sort=descending] button').dataset.k!=='salary') throw new Error('sort');
$('#pager button[aria-label="Page 3"]').click();const row=$$('#rows tr')[3];return row.cells[0].textContent+' ('+row.cells[1].textContent+', '+row.cells[5].textContent+')';`,
    wrongReplies: ['E-1062', 'E-1018', 'The ID is E-10321'],
  },
  'product-filter': {
    wrong: [`$('input[name=cat][value=Headphones]').click();$('#fw').click();$('#fa').click();setVal($('#sort'),'price-asc');await sleep(100);const b=$('[data-add]');const n=b.closest('article').querySelector('h3').textContent;b.click();await sleep(400);return n;`],
    right: [`$('input[name=cat][value=Headphones]').click();$('#fw').click();$('#fa').click();setVal($('#fr'),'4.5');setVal($('#sort'),'price-asc');await sleep(100);const b=$('[data-add]');const n=b.closest('article').querySelector('h3').textContent;b.click();await waitFor(()=>$('#cart li'));return n;`],
  },
  'infinite-scroll': (() => {
    const find = (needle) => `let a;for(let i=0;i<40&&!a;i++){scrollTo(0,document.body.scrollHeight);await sleep(300);a=$$('article').find(x=>x.textContent.includes(${JSON.stringify(needle)}));}
if(!a) throw new Error('post not found');const n=$$('article').length;a.querySelector('.like').click();await waitFor(()=>a.querySelector('.like').getAttribute('aria-pressed')==='true');return 'liked after loading '+n+' posts';`;
    return { wrong: [find('Glaze experiments')], right: [`if($$('article').some(x=>x.textContent.includes('pottery studio, opens'))) throw new Error('target visible without scrolling');` + find('pottery studio, opens')] };
  })(),
  kanban: (() => {
    const mv = `const move=async(title,to)=>{const card=await waitFor(()=>byText('.kcard h3',title)?.closest('.kcard'));card.querySelector('.more').click();const m=await waitFor(()=>card.querySelector('[data-move]'));m.click();
(await waitFor(()=>card.querySelector('[data-to="'+to+'"]'))).click();await waitFor(()=>$$('section[aria-label="'+to+'"] .kcard h3').some(h=>h.textContent===title));};`;
    return {
      wrong: [mv + `await move('Migrate billing cron jobs','Review');await move('Update onboarding copy','Done');return 'moved';`],
      right: [mv + `await move('Migrate billing webhooks','Review');await move('Update onboarding copy','Done');return 'moved';`],
    };
  })(),
  'settings-toggles': {
    wrong: [`if(!$('#save').disabled) throw new Error('save enabled before change');$('[data-k=email_digest]').click();$('[data-k=email_product]').click();$('[data-k=email_mentions]').click();$('#save').click();await waitFor(()=>$('#save').disabled);return 'saved';`],
    right: [`if(!$('#save').disabled) throw new Error('save enabled before change');$('[data-k=email_digest]').click();$('[data-k=email_product]').click();$('[data-k=push_mentions]').click();if($('#save').disabled) throw new Error('save still disabled');$('#save').click();await waitFor(()=>$('#save').disabled);return 'saved';`],
  },
  'delete-dialog': (() => {
    const rv = (n) => `$('button.revoke[data-n="${n}"]').click();const d=await waitFor(()=>$('[role=alertdialog]'));if(!$('#go').disabled) throw new Error('confirm not required');setVal($('#conf'),'${n}');$('#go').click();await waitFor(()=>!$('[role=alertdialog]'));return 'revoked';`;
    return { wrong: [rv('staging-ci-legacy')], right: [rv('staging-ci')] };
  })(),
  'tabs-accordion': (() => {
    const go = (t) => `if($('#retry').offsetParent) throw new Error('field visible initially');$('#t-adv').click();$('#b-hooks').click();setVal($('#retry'),'7');setVal($('#timeout'),'${t}');$('#saveHooks').click();await sleep(300);return 'saved';`;
    return { wrong: [go('10')], right: [go('25')] };
  })(),
  'icon-buttons': (() => {
    const go = (id) => `$('tr[data-id=${id}] [data-a=edit]').click();const s=await waitFor(()=>$('tr[data-id=${id}] select'));setVal(s,'Admin');$('tr[data-id=${id}] [data-a=save]').click();await waitFor(()=>!$('tr[data-id=${id}] select'));return $('tr[data-id=${id}]').textContent;`;
    return { wrong: [go('m2')], right: [go('m4')] };
  })(),
  'autocomplete-search': (() => {
    const go = (q, name) => [`setVal($('#q'),'${q}');const li=await waitFor(()=>$('#sugg li[data-n="${name}"]'));return later(()=>li.click());`,
      `const b=await waitFor(()=>$('#star'));if($('h1').textContent!=='${name}') throw new Error('wrong page');b.click();await waitFor(()=>b.getAttribute('aria-pressed')==='true');await sleep(200);return location.pathname;`];
    return {
      wrong: [`setVal($('#q'),'tiny-date');await waitFor(()=>$$('#sugg li').length);if($('#sugg li[data-n="tiny-date-fns"]')) throw new Error('target should need a longer query');return 'ok';`, ...go('tinydate', 'tinydate-fns')],
      right: go('tiny-date-fns', 'tiny-date-fns'),
    };
  })(),
  'radio-cards': (() => {
    const go = (region) => `setVal($('#wname'),'Helix Labs');const pick=(n,v)=>$('input[name='+n+'][value='+v+']').closest('label').click();
if(!$('input[name=support][value=priority]').disabled) throw new Error('priority should start disabled');
pick('plan','growth');pick('billing','annual');pick('region','${region}');pick('support','priority');if(!$('input[name=support][value=priority]').checked) throw new Error('priority not checked');
$('#f button[type=submit]').click();await waitFor(()=>!$('#done').hidden);return $('#done').textContent;`;
    return { wrong: [go('us-east')], right: [go('eu-central')] };
  })(),
  'spa-route': (() => {
    const go = (id) => `window.__noReload=1;await waitFor(()=>$('#view h1'));$('.side a[href="/t/spa-route/invoices"]').click();const a=await waitFor(()=>$('#view a[href$="/${id}"]'));a.click();
const pm=await waitFor(()=>$('#pm'));setVal(pm,'bank_transfer');$('#pay').click();await waitFor(()=>$('#view').textContent.includes('Paid by'));if(window.__noReload!==1) throw new Error('page reloaded');return location.pathname;`;
    return { wrong: [go('INV-2430')], right: [go('INV-2043')] };
  })(),
  'split-text-answer': {
    extract: `const ord=$('h1 .ordno').textContent.trim();let kg=0;for(const tr of $$('table tbody tr')){if(tr.cells[4].textContent.includes('Denver'))kg+=+tr.cells[2].textContent*parseFloat(tr.cells[3].textContent);}return ord+', '+kg.toFixed(1)+' kg';`,
    wrongReplies: ['ORD-58371, 14.6 kg', 'ORD-58317, 23.5 kg', 'ORD-58317, 114.6 kg', 'ORD 583 17, 14.6 kg'],
  },
};

async function openFresh(path) {
  await frob('open', 'about:blank');
  await frob('open', BASE + path);
  for (let i = 0; i < 100; i++) {
    try { if (await js(`return location.href===${JSON.stringify(BASE + path)}&&document.readyState==='complete'`)) { await sleep(150); return; } } catch {}
    await sleep(100);
  }
  throw new Error('page did not load: ' + path);
}

async function runSteps(path, steps) {
  await openFresh(path);
  const outs = [];
  for (const step of steps) { outs.push(await js(step)); await sleep(400); }
  return outs;
}

const results = [];
const server = await startServer(PORT);
try {
  const ids = tasks.map((t) => t.id);
  const missing = TASK_IDS.filter((x) => !ids.includes(x));
  if (missing.length) throw new Error('tasks.json missing ' + missing.join(', '));
  for (const task of tasks) {
    if (ONLY?.length && !ONLY.includes(task.id)) continue;
    const spec = S[task.id];
    const fails = [];
    const t0 = Date.now();
    try {
      if (!spec) throw new Error('no selftest spec');
      await reset(task.id);
      const s0 = await state(task.id);
      if (s0.pass !== false) fails.push('initial state not pass:false');
      // Snapshot sanity: the start page renders and exposes interactive elements.
      await openFresh(task.start_path);
      const snap = await frob('snapshot', '-i');
      if (!/ref=e\d+|@e\d+/.test(snap)) fails.push('snapshot has no refs');
      if (task.check.type === 'state') {
        await runSteps(task.start_path, spec.wrong);
        const sw = await pollState(task.id, true, 1200);
        if (sw.pass !== false) fails.push('wrong attempt passed: ' + sw.detail);
        await reset(task.id);
        if ((await state(task.id)).pass !== false) fails.push('reset did not clear');
        const outs = await runSteps(task.start_path, spec.right);
        const sr = await pollState(task.id, true);
        if (sr.pass !== true) fails.push(`right attempt failed: ${sr.detail} (steps: ${JSON.stringify(outs)})`);
        else spec.note = sr.detail;
      } else {
        const re = new RegExp(task.check.pattern, 'i');
        const [reply] = await runSteps(task.start_path, [spec.extract]);
        if (!re.test(String(reply))) fails.push(`pattern does not match extracted answer ${JSON.stringify(reply)}`);
        for (const w of spec.wrongReplies) if (re.test(w)) fails.push(`pattern matches wrong reply ${JSON.stringify(w)}`);
        spec.note = 'answer: ' + reply;
      }
    } catch (e) {
      fails.push(String(e.message || e).split('\n')[0].slice(0, 300));
    }
    const ok = !fails.length;
    results.push({ id: task.id, ok });
    console.log(`${ok ? 'PASS' : 'FAIL'} ${task.id.padEnd(22)} ${((Date.now() - t0) / 1000).toFixed(1)}s  ${ok ? spec.note || '' : fails.join(' | ')}`);
  }
} finally {
  try { await frob('close'); } catch {}
  server.close();
}
const failed = results.filter((r) => !r.ok);
console.log(`\n${results.length - failed.length}/${results.length} tasks passed self-test`);
process.exit(failed.length ? 1 : 0);
