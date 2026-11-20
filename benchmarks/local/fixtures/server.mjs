// Self-hosted fixture site for the local browser-agent benchmark.
// Zero dependencies. Usage: node server.mjs [--port 4517]

import { createServer } from 'node:http';
import { fileURLToPath } from 'node:url';

// ---------------------------------------------------------------- utilities

function mulberry32(seed) {
  return function () {
    seed |= 0; seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

const BASE_CSS = `
:root{--bg:#f5f6f8;--card:#fff;--text:#1d2330;--muted:#667085;--border:#e2e5eb;--accent:#4f46e5;--accent-soft:#eef2ff;--danger:#dc2626;--ok:#15803d}
*{box-sizing:border-box}
body{margin:0;font:14px/1.5 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;background:var(--bg);color:var(--text)}
header.top{display:flex;align-items:center;gap:20px;padding:0 24px;height:56px;background:#fff;border-bottom:1px solid var(--border)}
header.top .brand{font-weight:700;font-size:16px;display:flex;align-items:center;gap:8px}
header.top .brand i{display:inline-block;width:22px;height:22px;border-radius:6px;background:linear-gradient(135deg,#6366f1,#22d3ee)}
header.top nav{display:flex;gap:16px}
header.top nav a{color:var(--muted);text-decoration:none}
header.top nav a.active{color:var(--text);font-weight:600}
header.top .spacer{flex:1}
.avatar{width:30px;height:30px;border-radius:50%;background:#c7d2fe;display:inline-flex;align-items:center;justify-content:center;font-size:12px;font-weight:600;color:#3730a3}
main{max-width:980px;margin:28px auto;padding:0 16px}
h1{font-size:22px;margin:0 0 4px}
h2{font-size:17px;margin:0 0 12px}
p.sub{color:var(--muted);margin:0 0 20px}
.card{background:var(--card);border:1px solid var(--border);border-radius:12px;padding:20px;margin-bottom:16px}
button,.btn{font:inherit;border:1px solid var(--border);background:#fff;color:var(--text);border-radius:8px;padding:8px 14px;cursor:pointer}
button:hover{background:#f9fafb}
button.primary,.btn.primary{background:var(--accent);border-color:var(--accent);color:#fff}
button.primary:hover{background:#4338ca}
button.danger{background:var(--danger);border-color:var(--danger);color:#fff}
button:disabled{opacity:.5;cursor:not-allowed}
button.icon{padding:6px;line-height:0;border-color:transparent;background:transparent}
button.icon:hover{background:#f1f5f9}
input[type=text],input[type=email],input[type=password],input[type=number],input[type=search],input:not([type]),select,textarea{font:inherit;width:100%;padding:8px 10px;border:1px solid #cfd4dc;border-radius:8px;background:#fff;color:var(--text)}
input:focus,select:focus,textarea:focus{outline:2px solid #a5b4fc;border-color:var(--accent)}
label.field{display:block;font-weight:500;margin:14px 0 4px}
.hint{color:var(--muted);font-size:12px}
.err{color:var(--danger);font-size:13px;min-height:0}
.ok{color:var(--ok)}
.row{display:flex;gap:12px}
.row>*{flex:1}
.actions{display:flex;gap:8px;justify-content:flex-end;margin-top:18px}
.sr-only{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0}
.backdrop{position:fixed;inset:0;background:rgba(15,23,42,.55);display:flex;align-items:center;justify-content:center;z-index:100}
.modal{background:#fff;border-radius:14px;padding:24px;max-width:460px;width:92%;box-shadow:0 20px 60px rgba(0,0,0,.25);position:relative}
table.data{width:100%;border-collapse:collapse;background:#fff}
table.data th,table.data td{text-align:left;padding:10px 12px;border-bottom:1px solid var(--border)}
table.data th{font-size:12px;text-transform:uppercase;letter-spacing:.03em;color:var(--muted);font-weight:600}
.badge{display:inline-block;padding:1px 8px;border-radius:999px;font-size:12px;background:#f1f5f9;color:#334155}
.badge.green{background:#dcfce7;color:#166534}.badge.amber{background:#fef3c7;color:#92400e}.badge.blue{background:#dbeafe;color:#1e40af}
.toast{position:fixed;bottom:20px;left:50%;transform:translateX(-50%);background:#111827;color:#fff;padding:10px 16px;border-radius:8px}
`;

// Client helpers injected into every page.
function commonClient() {
  window.api = async function (path, body, method) {
    const r = await fetch(path, {
      method: method || (body === undefined ? 'GET' : 'POST'),
      headers: { 'content-type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    let data = null;
    try { data = await r.json(); } catch {}
    return { ok: r.ok, status: r.status, data };
  };
  window.h = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  window.toast = (msg) => {
    const t = document.createElement('div');
    t.className = 'toast'; t.setAttribute('role', 'status'); t.textContent = msg;
    document.body.appendChild(t); setTimeout(() => t.remove(), 2500);
  };
}

// Method shorthand (`script() {}`) stringifies without the `function` keyword.
function fnSource(f) {
  const src = String(f);
  return /^(async\s+)?(function\b|\(|[\w$]+\s*=>)/.test(src) ? src : 'function ' + src;
}

function page({ title, app = 'Acme Cloud', nav = [], body, css = '', script, data, user = 'MC' }) {
  const navHtml = nav.map(([label, href, active]) => `<a href="${href}"${active ? ' class="active" aria-current="page"' : ''}>${esc(label)}</a>`).join('');
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>${esc(title)}</title><style>${BASE_CSS}${css}</style></head><body>
<header class="top"><span class="brand"><i aria-hidden="true"></i>${esc(app)}</span><nav aria-label="Primary">${navHtml}</nav><span class="spacer"></span><span class="avatar" aria-hidden="true">${esc(user)}</span></header>
${body}
<script>(${commonClient})();</script>
${script ? `<script>(${fnSource(script)})(${JSON.stringify(data ?? null)});</script>` : ''}
</body></html>`;
}

const html = (body) => ({ status: 200, type: 'text/html; charset=utf-8', body });
const json = (obj, status = 200) => ({ status, type: 'application/json', body: JSON.stringify(obj) });
const redirect = (loc) => ({ status: 302, headers: { location: loc }, body: '' });

const ICON = {
  edit: '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>',
  trash: '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M19 6l-1 14H6L5 6"/></svg>',
  check: '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" aria-hidden="true"><path d="M20 6 9 17l-5-5"/></svg>',
  x: '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12"/></svg>',
  dots: '<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><circle cx="5" cy="12" r="2"/><circle cx="12" cy="12" r="2"/><circle cx="19" cy="12" r="2"/></svg>',
  heart: '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1-1.1a5.5 5.5 0 0 0-7.8 7.8L12 21l8.8-8.6a5.5 5.5 0 0 0 0-7.8Z"/></svg>',
};

// ---------------------------------------------------------------- tasks
// Each task: { init() -> state, page(sub, ctx), api(method, sub, body, ctx), check(state) -> {pass, detail} }

const T = {};

// 1. login-validation ------------------------------------------------------
T['login-validation'] = {
  init: () => ({ loggedIn: false, attempts: [] }),
  page(sub, { state }) {
    if (sub === 'home') {
      if (!state.loggedIn) return redirect('/t/login-validation/');
      return html(page({ title: 'Dashboard - Acme Console', app: 'Acme Console', nav: [['Dashboard', '#', true], ['Projects', '#'], ['Billing', '#']],
        body: `<main><h1>Welcome back, Maria</h1><p class="sub">You are signed in as maria.chen@acme.test.</p><div class="card">No incidents in the last 24 hours.</div></main>` }));
    }
    return html(page({
      title: 'Sign in - Acme Console', app: 'Acme Console', user: '?',
      css: `.login{max-width:380px;margin:60px auto}.login .err.banner{background:#fef2f2;border:1px solid #fecaca;padding:10px 12px;border-radius:8px;margin-bottom:8px}.login .err.banner:empty{display:none}`,
      body: `<main class="login"><div class="card"><h1>Sign in</h1><p class="sub">Use your Acme work account.</p>
<div id="err" class="err banner" role="alert"></div>
<form id="f" novalidate>
<label class="field" for="email">Email</label><input id="email" name="email" type="email" autocomplete="username" aria-describedby="emailErr"><div id="emailErr" class="err"></div>
<label class="field" for="password">Password</label><input id="password" name="password" type="password" autocomplete="current-password" aria-describedby="pwErr"><div id="pwErr" class="err"></div>
<div id="termsWrap" hidden style="margin-top:14px;padding:12px;border:1px solid var(--border);border-radius:8px;background:#fafafa">
<p style="margin:0 0 8px">We updated our Terms of Service on March 1. <a href="#">Read the changes</a>.</p>
<label><input type="checkbox" name="terms" id="terms"> I accept the updated Terms of Service</label></div>
<div class="actions"><button class="primary" type="submit" style="width:100%">Sign in</button></div>
<p class="hint" style="text-align:center"><a href="#">Forgot password?</a></p>
</form></div></main>`,
      script() {
        const f = document.getElementById('f'), err = document.getElementById('err'), tw = document.getElementById('termsWrap');
        f.addEventListener('submit', async (e) => {
          e.preventDefault(); err.textContent = '';
          const email = f.email.value.trim(), password = f.password.value;
          const ee = document.getElementById('emailErr'), pe = document.getElementById('pwErr');
          ee.textContent = ''; pe.textContent = ''; f.email.removeAttribute('aria-invalid'); f.password.removeAttribute('aria-invalid');
          let bad = false;
          if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) { ee.textContent = 'Enter a valid email address.'; f.email.setAttribute('aria-invalid', 'true'); bad = true; }
          if (password.length < 8) { pe.textContent = 'Password must be at least 8 characters.'; f.password.setAttribute('aria-invalid', 'true'); bad = true; }
          if (bad) return;
          const r = await api('/api/login-validation/login', { email, password, terms: !tw.hidden && f.terms.checked });
          if (r.ok) { location.href = '/t/login-validation/home'; return; }
          err.textContent = r.data.error;
          if (r.data.needTerms) { tw.hidden = false; f.terms.focus(); }
        });
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub !== 'login') return null;
    state.attempts.push({ email: body.email });
    if (String(body.email).toLowerCase() !== 'maria.chen@acme.test' || body.password !== 'harbor-42') return json({ error: 'Incorrect email or password.' }, 401);
    if (!body.terms) return json({ error: 'Our Terms of Service changed. Review and accept them to continue.', needTerms: true }, 409);
    state.loggedIn = true;
    return json({ ok: true });
  },
  check: (s) => ({ pass: s.loggedIn, detail: s.loggedIn ? 'signed in' : `not signed in (${s.attempts.length} attempts)` }),
};

// 2. checkout-wizard -------------------------------------------------------
const CHECKOUT_EXPECT = { name: 'Jonas Weber', street: '742 Evergreen Terrace', city: 'Springfield', state: 'OR', zip: '97477', shipping: 'express', card: '4242424242424242', exp: '08/29', cvc: '314', agree: true };
T['checkout-wizard'] = {
  init: () => ({ orders: [] }),
  page() {
    return html(page({
      title: 'Checkout - Trailhead Outfitters', app: 'Trailhead Outfitters', user: 'JW',
      css: `.steps{display:flex;gap:8px;margin-bottom:16px;list-style:none;padding:0}.steps li{flex:1;padding:8px 10px;border-radius:8px;background:#eef0f4;color:var(--muted);font-weight:500}.steps li[aria-current]{background:var(--accent-soft);color:var(--accent)}
.grid{display:grid;grid-template-columns:1fr 280px;gap:16px}.ship-opt{display:flex;gap:10px;align-items:center;border:1px solid var(--border);border-radius:8px;padding:10px;margin:6px 0}.summary dt{color:var(--muted)}.summary dd{margin:0 0 8px}`,
      body: `<main><h1>Checkout</h1><p class="sub">Complete your order in three steps.</p>
<ol class="steps" aria-label="Checkout progress"><li id="s1">1. Shipping</li><li id="s2">2. Payment</li><li id="s3">3. Review</li></ol>
<div class="grid"><div>
<section class="card" id="step1" aria-labelledby="h1"><h2 id="h1">Shipping address</h2>
<label class="field" for="name">Full name</label><input id="name">
<label class="field" for="street">Street address</label><input id="street">
<div class="row"><div><label class="field" for="city">City</label><input id="city"></div>
<div><label class="field" for="state">State</label><select id="state"><option value="">Select…</option><option value="CA">California</option><option value="CO">Colorado</option><option value="IL">Illinois</option><option value="NY">New York</option><option value="OH">Ohio</option><option value="OR">Oregon</option><option value="TX">Texas</option><option value="WA">Washington</option></select></div>
<div><label class="field" for="zip">ZIP code</label><input id="zip" inputmode="numeric"></div></div>
<fieldset style="border:0;padding:0;margin:14px 0 0"><legend style="font-weight:500">Shipping method</legend>
<label class="ship-opt"><input type="radio" name="ship" value="standard" checked> Standard (5 to 7 days) <span style="margin-left:auto">Free</span></label>
<label class="ship-opt"><input type="radio" name="ship" value="express"> Express (2 days) <span style="margin-left:auto">$14.99</span></label>
<label class="ship-opt"><input type="radio" name="ship" value="overnight"> Overnight <span style="margin-left:auto">$29.99</span></label></fieldset>
<div id="e1" class="err" role="alert"></div>
<div class="actions"><button class="primary" id="next1">Continue to payment</button></div></section>
<section class="card" id="step2" hidden aria-labelledby="h2"><h2 id="h2">Payment</h2>
<label class="field" for="card">Card number</label><input id="card" inputmode="numeric" placeholder="1234 1234 1234 1234">
<div class="row"><div><label class="field" for="exp">Expiry (MM/YY)</label><input id="exp" placeholder="MM/YY"></div><div><label class="field" for="cvc">Security code</label><input id="cvc" inputmode="numeric"></div></div>
<label style="display:block;margin-top:14px"><input type="checkbox" id="sameBilling" checked> Billing address same as shipping</label>
<div id="e2" class="err" role="alert"></div>
<div class="actions"><button id="back2">Back</button><button class="primary" id="next2">Review order</button></div></section>
<section class="card" id="step3" hidden aria-labelledby="h3"><h2 id="h3">Review your order</h2>
<dl class="summary" id="summary"></dl>
<label style="display:block;margin-top:10px"><input type="checkbox" id="agree"> I agree to the Terms of Sale and Return Policy</label>
<div id="e3" class="err" role="alert"></div>
<div class="actions"><button id="back3">Back</button><button class="primary" id="place">Place order</button></div></section>
<section class="card" id="done" hidden><h2>Thank you!</h2><p id="doneMsg"></p></section>
</div>
<aside class="card" aria-label="Order summary"><h2>Your cart</h2><p>Trail Runner GTX, size 10 <span style="float:right">$129.00</span></p><p>Merino hiking socks (2) <span style="float:right">$32.00</span></p><hr><p><b>Subtotal</b><span style="float:right">$161.00</span></p></aside></div></main>`,
      script() {
        const $ = (id) => document.getElementById(id);
        const show = (n) => { ['step1', 'step2', 'step3'].forEach((s, i) => { $(s).hidden = i + 1 !== n; $('s' + (i + 1)).toggleAttribute('aria-current', i + 1 === n); if (i + 1 === n) $('s' + (i + 1)).setAttribute('aria-current', 'step'); }); };
        show(1);
        const ship = () => document.querySelector('input[name=ship]:checked').value;
        $('next1').onclick = () => {
          const miss = [['name', 'Full name'], ['street', 'Street address'], ['city', 'City'], ['state', 'State'], ['zip', 'ZIP code']].filter(([id]) => !$(id).value.trim()).map(([, l]) => l);
          if (miss.length) { $('e1').textContent = 'Please fill in: ' + miss.join(', ') + '.'; return; }
          if (!/^\d{5}$/.test($('zip').value.trim())) { $('e1').textContent = 'ZIP code must be 5 digits.'; return; }
          $('e1').textContent = ''; show(2);
        };
        $('back2').onclick = () => show(1);
        $('next2').onclick = () => {
          const c = $('card').value.replace(/\s/g, '');
          if (!/^\d{16}$/.test(c)) { $('e2').textContent = 'Enter a valid 16 digit card number.'; return; }
          if (!/^(0[1-9]|1[0-2])\/\d{2}$/.test($('exp').value.trim())) { $('e2').textContent = 'Expiry must look like MM/YY.'; return; }
          if (!/^\d{3}$/.test($('cvc').value.trim())) { $('e2').textContent = 'Security code must be 3 digits.'; return; }
          $('e2').textContent = '';
          const st = $('state'); const shipLabel = { standard: 'Standard', express: 'Express', overnight: 'Overnight' }[ship()];
          $('summary').innerHTML = `<dt>Ship to</dt><dd>${h($('name').value)}, ${h($('street').value)}, ${h($('city').value)}, ${h(st.options[st.selectedIndex].text)} ${h($('zip').value)}</dd><dt>Shipping</dt><dd>${shipLabel}</dd><dt>Payment</dt><dd>Card ending ${c.slice(-4)}, expires ${h($('exp').value)}</dd>`;
          show(3);
        };
        $('back3').onclick = () => show(2);
        $('place').onclick = async () => {
          if (!$('agree').checked) { $('e3').textContent = 'You must agree to the Terms of Sale to place your order.'; return; }
          const r = await api('/api/checkout-wizard/order', { name: $('name').value.trim(), street: $('street').value.trim(), city: $('city').value.trim(), state: $('state').value, zip: $('zip').value.trim(), shipping: ship(), card: $('card').value.replace(/\s/g, ''), exp: $('exp').value.trim(), cvc: $('cvc').value.trim(), agree: $('agree').checked });
          ['step1', 'step2', 'step3'].forEach((s) => ($(s).hidden = true));
          $('done').hidden = false; $('doneMsg').textContent = 'Order ' + r.data.orderId + ' has been placed.';
        };
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub !== 'order') return null;
    state.orders.push(body);
    return json({ orderId: 'TH-' + (10400 + state.orders.length) });
  },
  check(s) {
    if (!s.orders.length) return { pass: false, detail: 'no order placed' };
    if (s.orders.length > 1) return { pass: false, detail: `${s.orders.length} orders placed, expected 1` };
    const o = s.orders[0];
    const bad = Object.keys(CHECKOUT_EXPECT).filter((k) => o[k] !== CHECKOUT_EXPECT[k]);
    return { pass: !bad.length, detail: bad.length ? 'mismatched: ' + bad.map((k) => `${k}=${JSON.stringify(o[k])}`).join(', ') : 'order correct' };
  },
};

// 3. cookie-consent --------------------------------------------------------
T['cookie-consent'] = {
  init: () => ({ subs: [], consent: null }),
  page() {
    return html(page({
      title: 'Pricing - Relay', app: 'Relay', nav: [['Product', '#'], ['Pricing', '#', true], ['Docs', '#']],
      css: `.plans{display:grid;grid-template-columns:repeat(3,1fr);gap:16px}.plan .price{font-size:28px;font-weight:700}.seg{display:inline-flex;border:1px solid var(--border);border-radius:999px;padding:3px;background:#fff;margin-bottom:20px}.seg button{border:0;border-radius:999px}.seg button[aria-pressed=true]{background:var(--accent);color:#fff}.plan ul{padding-left:18px;color:var(--muted)}`,
      body: `<div id="app"><main><h1>Simple pricing for growing teams</h1><p class="sub">Start free. Upgrade when you need more.</p>
<div class="seg" role="group" aria-label="Billing period"><button aria-pressed="true" data-b="monthly">Monthly</button><button aria-pressed="false" data-b="annual">Annual (save 20%)</button></div>
<div class="plans">
<div class="card plan"><h2>Starter</h2><div class="price" data-m="0" data-a="0">$0</div><ul><li>3 projects</li><li>Community support</li></ul><button data-plan="starter">Choose Starter</button></div>
<div class="card plan"><h2>Team</h2><div class="price" data-m="24" data-a="19">$24</div><ul><li>Unlimited projects</li><li>SSO</li><li>Email support</li></ul><button class="primary" data-plan="team">Choose Team</button></div>
<div class="card plan"><h2>Business</h2><div class="price" data-m="59" data-a="47">$59</div><ul><li>Audit logs</li><li>99.9% SLA</li></ul><button data-plan="business">Choose Business</button></div>
</div><div id="result" role="status" class="card" hidden></div></main></div>
<div class="backdrop" id="consent"><div class="modal" role="dialog" aria-modal="true" aria-labelledby="ch"><h2 id="ch">We value your privacy</h2>
<p>We use cookies to improve your experience, measure traffic and personalise content. You can accept all cookies or keep only the essential ones.</p>
<div class="actions" style="justify-content:flex-start;flex-wrap:wrap"><button class="primary" data-c="all">Accept all</button><button data-c="essential">Reject non-essential</button><button data-c="custom">Customize</button></div></div></div>`,
      script() {
        const app = document.getElementById('app'), consent = document.getElementById('consent');
        let billing = 'monthly';
        app.inert = true;
        consent.querySelectorAll('button').forEach((b) => (b.onclick = async () => {
          await api('/api/cookie-consent/consent', { choice: b.dataset.c });
          consent.remove(); app.inert = false;
        }));
        document.querySelectorAll('.seg button').forEach((b) => (b.onclick = () => {
          if (document.getElementById('consent')) return;
          billing = b.dataset.b;
          document.querySelectorAll('.seg button').forEach((x) => x.setAttribute('aria-pressed', String(x === b)));
          document.querySelectorAll('.price').forEach((p) => (p.textContent = '$' + (billing === 'annual' ? p.dataset.a : p.dataset.m)));
        }));
        document.querySelectorAll('[data-plan]').forEach((b) => (b.onclick = async () => {
          if (document.getElementById('consent')) return;
          await api('/api/cookie-consent/subscribe', { plan: b.dataset.plan, billing });
          const r = document.getElementById('result'); r.hidden = false;
          r.textContent = `Subscription started: ${b.dataset.plan[0].toUpperCase() + b.dataset.plan.slice(1)} plan, billed ${billing}.`;
        }));
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub === 'consent') { state.consent = body.choice; return json({ ok: true }); }
    if (sub === 'subscribe') { state.subs.push({ plan: body.plan, billing: body.billing }); return json({ ok: true }); }
    return null;
  },
  check(s) {
    const last = s.subs.at(-1);
    if (!last) return { pass: false, detail: 'no subscription' };
    const pass = s.subs.length === 1 && last.plan === 'team' && last.billing === 'annual';
    return { pass, detail: `subscriptions: ${s.subs.map((x) => x.plan + '/' + x.billing).join(', ')}` };
  },
};

// 4. newsletter-popup ------------------------------------------------------
T['newsletter-popup'] = {
  init: () => ({ tickets: [], newsletter: [] }),
  page() {
    return html(page({
      title: 'Contact support - Brightdesk', app: 'Brightdesk Help Center', nav: [['Articles', '#'], ['Contact us', '#', true]],
      css: `.promo h2{font-size:22px}.promo .close{position:absolute;top:10px;right:10px}`,
      body: `<div id="app"><main><h1>Contact support</h1><p class="sub">Our team usually replies within one business day.</p>
<form class="card" id="f" novalidate>
<label class="field" for="subject">Subject</label><input id="subject">
<label class="field" for="category">Category</label><select id="category"><option value="">Choose a category</option><option value="account">Account and login</option><option value="billing">Billing and invoices</option><option value="technical">Technical issue</option></select>
<fieldset style="border:0;padding:0;margin:14px 0 0"><legend style="font-weight:500">Priority</legend>
<label><input type="radio" name="priority" value="low"> Low</label> &nbsp; <label><input type="radio" name="priority" value="normal" checked> Normal</label> &nbsp; <label><input type="radio" name="priority" value="high"> High</label></fieldset>
<label class="field" for="message">Message</label><textarea id="message" rows="5"></textarea>
<div id="e" class="err" role="alert"></div>
<div class="actions"><button class="primary" type="submit">Send message</button></div></form>
<div id="sent" class="card" role="status" hidden></div></main></div>`,
      script() {
        const app = document.getElementById('app'), f = document.getElementById('f');
        setTimeout(() => {
          const d = document.createElement('div');
          d.className = 'backdrop'; d.id = 'promo';
          d.innerHTML = `<div class="modal promo" role="dialog" aria-modal="true" aria-labelledby="ph"><button class="icon close" aria-label="Close">${'<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12"/></svg>'}</button>
<h2 id="ph">Get 20% off Brightdesk Pro</h2><p>Join 40,000 support leaders who get our weekly tips. Unsubscribe anytime.</p>
<form id="nf"><label class="field" for="nemail">Work email</label><input id="nemail" type="email"><div class="actions"><button class="primary" type="submit">Subscribe</button></div></form></div>`;
          document.body.appendChild(d); app.inert = true;
          const close = () => { d.remove(); app.inert = false; };
          d.querySelector('.close').onclick = close;
          d.addEventListener('click', (e) => { if (e.target === d) close(); });
          d.querySelector('#nf').onsubmit = async (e) => { e.preventDefault(); await api('/api/newsletter-popup/newsletter', { email: d.querySelector('#nemail').value }); close(); toast('Thanks for subscribing!'); };
        }, 2000);
        f.onsubmit = async (e) => {
          e.preventDefault();
          if (document.getElementById('promo')) return;
          const body = { subject: f.subject.value.trim(), category: f.category.value, priority: f.querySelector('input[name=priority]:checked').value, message: f.message.value.trim() };
          const miss = ['subject', 'category', 'message'].filter((k) => !body[k]);
          const err = document.getElementById('e');
          if (miss.length) { err.textContent = 'Please complete: ' + miss.join(', ') + '.'; return; }
          err.textContent = '';
          const r = await api('/api/newsletter-popup/ticket', body);
          f.hidden = true; const s = document.getElementById('sent'); s.hidden = false; s.textContent = `Thanks! Ticket #${r.data.id} was created.`;
        };
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub === 'newsletter') { state.newsletter.push(body.email); return json({ ok: true }); }
    if (sub === 'ticket') { state.tickets.push(body); return json({ id: 58200 + state.tickets.length }); }
    return null;
  },
  check(s) {
    if (s.tickets.length !== 1) return { pass: false, detail: `${s.tickets.length} tickets` };
    const t = s.tickets[0];
    const bad = [];
    if (t.subject !== 'Invoice shows wrong VAT number') bad.push('subject=' + JSON.stringify(t.subject));
    if (t.category !== 'billing') bad.push('category=' + t.category);
    if (t.priority !== 'high') bad.push('priority=' + t.priority);
    if (!String(t.message).includes('DE298374651')) bad.push('message lacks VAT number');
    if (s.newsletter.length) bad.push('signed up for newsletter');
    return { pass: !bad.length, detail: bad.length ? bad.join('; ') : 'ticket correct' };
  },
};

// 5. combobox --------------------------------------------------------------
const COUNTRIES = [['AR', 'Argentina'], ['AU', 'Australia'], ['AT', 'Austria'], ['BE', 'Belgium'], ['BR', 'Brazil'], ['CA', 'Canada'], ['CL', 'Chile'], ['CN', 'China'], ['CO', 'Colombia'], ['CZ', 'Czechia'], ['DK', 'Denmark'], ['EG', 'Egypt'], ['FI', 'Finland'], ['FR', 'France'], ['DE', 'Germany'], ['GR', 'Greece'], ['IN', 'India'], ['ID', 'Indonesia'], ['IE', 'Ireland'], ['IL', 'Israel'], ['IT', 'Italy'], ['JP', 'Japan'], ['KE', 'Kenya'], ['MY', 'Malaysia'], ['MX', 'Mexico'], ['NP', 'Nepal'], ['NL', 'Netherlands'], ['NC', 'New Caledonia'], ['NZ', 'New Zealand'], ['NI', 'Nicaragua'], ['NE', 'Niger'], ['NG', 'Nigeria'], ['NO', 'Norway'], ['PE', 'Peru'], ['PH', 'Philippines'], ['PL', 'Poland'], ['PT', 'Portugal'], ['RO', 'Romania'], ['SG', 'Singapore'], ['ZA', 'South Africa'], ['KR', 'South Korea'], ['ES', 'Spain'], ['SE', 'Sweden'], ['CH', 'Switzerland'], ['TH', 'Thailand'], ['TR', 'Turkey'], ['UA', 'Ukraine'], ['GB', 'United Kingdom'], ['US', 'United States'], ['VN', 'Vietnam']];
T['combobox'] = {
  init: () => ({ saves: [] }),
  page() {
    return html(page({
      title: 'Profile - Lattice', app: 'Lattice', nav: [['Home', '#'], ['Profile', '#', true]], user: 'RK',
      css: `.combo{position:relative}.combo input{padding-right:34px}.combo .tog{position:absolute;right:4px;top:4px;padding:4px 8px;border:0;background:transparent}
.combo ul{position:absolute;left:0;right:0;top:100%;margin:4px 0 0;padding:4px;list-style:none;background:#fff;border:1px solid var(--border);border-radius:8px;max-height:220px;overflow:auto;box-shadow:0 8px 24px rgba(0,0,0,.12);z-index:5}
.combo li{padding:6px 10px;border-radius:6px;cursor:pointer}.combo li[aria-selected=true],.combo li:hover{background:var(--accent-soft)}.combo li.empty{color:var(--muted);cursor:default}`,
      body: `<main><h1>Your profile</h1><p class="sub">This information appears on your public profile.</p>
<div class="card" style="max-width:560px">
<label class="field" for="dn">Display name</label><input id="dn" value="Rhea Kapoor">
<label class="field" id="country-label" for="country">Country</label>
<div class="combo"><input id="country" role="combobox" aria-autocomplete="list" aria-expanded="false" aria-controls="country-list" autocomplete="off" value="United States">
<button class="tog" type="button" tabindex="-1" aria-label="Show all countries">▾</button>
<ul id="country-list" role="listbox" aria-labelledby="country-label" hidden></ul></div>
<div class="hint">Start typing to search.</div>
<label class="field" for="bio">Bio</label><textarea id="bio" rows="3">Product designer. Coffee, maps, and bicycles.</textarea>
<div id="e" class="err" role="alert"></div>
<div class="actions"><button class="primary" id="save">Save profile</button></div></div></main>`,
      data: COUNTRIES,
      script(countries) {
        const input = document.getElementById('country'), list = document.getElementById('country-list');
        let selected = 'US', items = [], active = -1;
        const open = (q) => {
          const ql = q.trim().toLowerCase();
          const starts = countries.filter(([, n]) => n.toLowerCase().startsWith(ql));
          const contains = countries.filter(([, n]) => !n.toLowerCase().startsWith(ql) && n.toLowerCase().includes(ql));
          items = ql ? starts.concat(contains) : countries.slice();
          active = -1;
          list.innerHTML = items.length ? items.map(([c, n], i) => `<li role="option" id="opt-${c}" aria-selected="false" data-i="${i}">${h(n)}</li>`).join('') : '<li class="empty">No matches</li>';
          list.hidden = false; input.setAttribute('aria-expanded', 'true'); input.removeAttribute('aria-activedescendant');
        };
        const close = () => { list.hidden = true; input.setAttribute('aria-expanded', 'false'); input.removeAttribute('aria-activedescendant'); };
        const setActive = (i) => {
          active = i;
          list.querySelectorAll('li[role=option]').forEach((li, j) => li.setAttribute('aria-selected', String(j === i)));
          const li = list.querySelector(`[data-i="${i}"]`);
          if (li) { input.setAttribute('aria-activedescendant', li.id); li.scrollIntoView({ block: 'nearest' }); }
        };
        const choose = (i) => { const [c, n] = items[i]; selected = c; input.value = n; close(); };
        input.addEventListener('input', () => { selected = null; open(input.value); });
        input.addEventListener('keydown', (e) => {
          if (e.key === 'ArrowDown') { e.preventDefault(); if (list.hidden) open(input.value); setActive(Math.min(active + 1, items.length - 1)); }
          else if (e.key === 'ArrowUp') { e.preventDefault(); setActive(Math.max(active - 1, 0)); }
          else if (e.key === 'Enter') { if (!list.hidden && active >= 0) { e.preventDefault(); choose(active); } }
          else if (e.key === 'Escape') close();
        });
        input.addEventListener('blur', () => setTimeout(close, 200));
        list.addEventListener('mousedown', (e) => e.preventDefault());
        list.addEventListener('click', (e) => { const li = e.target.closest('li[role=option]'); if (li) choose(+li.dataset.i); });
        document.querySelector('.tog').onclick = () => (list.hidden ? (open(''), input.focus()) : close());
        document.getElementById('save').onclick = async () => {
          const err = document.getElementById('e');
          if (!selected) { err.textContent = 'Choose a country from the list.'; return; }
          err.textContent = '';
          await api('/api/combobox/save', { displayName: document.getElementById('dn').value, country: selected, bio: document.getElementById('bio').value });
          toast('Profile saved');
        };
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub !== 'save') return null;
    state.saves.push(body); return json({ ok: true });
  },
  check(s) {
    const last = s.saves.at(-1);
    if (!last) return { pass: false, detail: 'profile not saved' };
    const pass = last.country === 'NZ' && last.displayName === 'Rhea Kapoor';
    return { pass, detail: `saved country=${last.country} displayName=${last.displayName}` };
  },
};

// 6. datepicker ------------------------------------------------------------
const TIMES = ['09:00', '09:30', '10:00', '11:00', '13:30', '14:00', '15:00', '15:30', '16:30'];
const slotsFor = (iso) => { const d = +iso.slice(8, 10), m = +iso.slice(5, 7); return TIMES.filter((t, i) => (d * 7 + m * 2 + i * 3) % 5 !== 0); };
T['datepicker'] = {
  init: () => ({ bookings: [] }),
  page() {
    return html(page({
      title: 'Book a consultation - Northstar Advisory', app: 'Northstar Advisory', user: 'AP',
      css: `.datebox{display:flex;gap:8px}.datebox input{flex:1;background:#f9fafb}
.cal{position:absolute;z-index:10;background:#fff;border:1px solid var(--border);border-radius:12px;padding:12px;box-shadow:0 10px 30px rgba(0,0,0,.15);width:300px}
.cal header{display:flex;align-items:center;justify-content:space-between;margin-bottom:8px}.cal table{width:100%;border-collapse:collapse}.cal th{font-size:11px;color:var(--muted);font-weight:600;padding:4px 0}
.cal td{text-align:center;padding:1px}.cal td button{width:36px;height:34px;padding:0;border:0;border-radius:8px}.cal td button[aria-selected=true]{background:var(--accent);color:#fff}.cal td button.today{outline:1px solid var(--accent)}
.slots{display:flex;flex-wrap:wrap;gap:8px}.slots [role=radio]{border:1px solid var(--border);border-radius:8px;padding:6px 12px;cursor:pointer;background:#fff}.slots [aria-checked=true]{background:var(--accent);color:#fff;border-color:var(--accent)}`,
      body: `<main><h1>Book a 30 minute consultation</h1><p class="sub">Today is <b>Tuesday, March 10, 2026</b>. Appointments are available on weekdays.</p>
<div class="card" style="max-width:560px;position:relative">
<label class="field" for="nm">Your name</label><input id="nm">
<label class="field" for="em">Email</label><input id="em" type="email">
<label class="field" for="date">Date</label>
<div class="datebox"><input id="date" readonly placeholder="No date selected"><button type="button" id="pick" aria-haspopup="dialog" aria-expanded="false">Choose date</button></div>
<div id="cal" class="cal" role="dialog" aria-label="Choose date" hidden></div>
<div class="field" style="font-weight:500;margin:14px 0 4px" id="slotsLabel">Available times</div>
<div id="slots" class="slots" role="radiogroup" aria-labelledby="slotsLabel"><span class="hint">Pick a date to see available times.</span></div>
<div id="e" class="err" role="alert"></div>
<div class="actions"><button class="primary" id="book">Book appointment</button></div></div>
<div id="done" class="card" role="status" hidden></div></main>`,
      script() {
        const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
        const DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
        const TODAY = { y: 2026, m: 2, d: 10 }; // fixed fake today (month is 0-based)
        let view = { y: 2026, m: 2 }, picked = null, time = null;
        const cal = document.getElementById('cal'), pick = document.getElementById('pick');
        const iso = (y, m, d) => `${y}-${String(m + 1).padStart(2, '0')}-${String(d).padStart(2, '0')}`;
        const dow = (y, m, d) => new Date(Date.UTC(y, m, d)).getUTCDay();
        const render = () => {
          const { y, m } = view; const first = dow(y, m, 1); const n = new Date(Date.UTC(y, m + 1, 0)).getUTCDate();
          let cells = '', row = '';
          for (let i = 0; i < first; i++) row += '<td></td>';
          for (let d = 1; d <= n; d++) {
            const wd = dow(y, m, d); const k = iso(y, m, d);
            const past = k < iso(TODAY.y, TODAY.m, TODAY.d) || k === iso(TODAY.y, TODAY.m, TODAY.d);
            const disabled = past || wd === 0 || wd === 6 || k > '2026-07-31';
            const cls = k === iso(TODAY.y, TODAY.m, TODAY.d) ? ' class="today"' : '';
            row += `<td><button type="button"${cls} data-k="${k}" aria-label="${DAYS[wd]}, ${MONTHS[m]} ${d}, ${y}" aria-selected="${picked === k}"${disabled ? ' disabled' : ''}>${d}</button></td>`;
            if (wd === 6) { cells += `<tr>${row}</tr>`; row = ''; }
          }
          if (row) cells += `<tr>${row}</tr>`;
          cal.innerHTML = `<header><button type="button" class="icon" id="prev" aria-label="Previous month">‹</button><b aria-live="polite">${MONTHS[m]} ${y}</b><button type="button" class="icon" id="next" aria-label="Next month">›</button></header>
<table role="grid"><thead><tr>${['Su', 'Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa'].map((x) => `<th scope="col">${x}</th>`).join('')}</tr></thead><tbody>${cells}</tbody></table>`;
          cal.querySelector('#prev').onclick = () => { if (view.y * 12 + view.m > TODAY.y * 12 + TODAY.m) { view.m--; if (view.m < 0) { view.m = 11; view.y--; } render(); } };
          cal.querySelector('#next').onclick = () => { view.m++; if (view.m > 11) { view.m = 0; view.y++; } render(); };
          cal.querySelectorAll('td button').forEach((b) => (b.onclick = () => choose(b.dataset.k, b.getAttribute('aria-label'))));
        };
        const setOpen = (o) => { cal.hidden = !o; pick.setAttribute('aria-expanded', String(o)); if (o) render(); };
        pick.onclick = () => setOpen(cal.hidden);
        document.addEventListener('keydown', (e) => { if (e.key === 'Escape') setOpen(false); });
        const choose = async (k, label) => {
          picked = k; time = null; document.getElementById('date').value = label; setOpen(false);
          const s = document.getElementById('slots'); s.innerHTML = '<span class="hint">Loading times…</span>';
          const r = await api('/api/datepicker/slots?date=' + k);
          s.innerHTML = r.data.slots.map((t) => { const [hh, mm] = t.split(':').map(Number); const lbl = `${((hh + 11) % 12) + 1}:${String(mm).padStart(2, '0')} ${hh < 12 ? 'AM' : 'PM'}`; return `<span role="radio" tabindex="0" aria-checked="false" data-t="${t}">${lbl}</span>`; }).join('');
          s.querySelectorAll('[role=radio]').forEach((el) => {
            const sel = () => { time = el.dataset.t; s.querySelectorAll('[role=radio]').forEach((x) => x.setAttribute('aria-checked', String(x === el))); };
            el.onclick = sel; el.onkeydown = (e) => { if (e.key === ' ' || e.key === 'Enter') { e.preventDefault(); sel(); } };
          });
        };
        document.getElementById('book').onclick = async () => {
          const err = document.getElementById('e');
          const body = { name: document.getElementById('nm').value.trim(), email: document.getElementById('em').value.trim(), date: picked, time };
          if (!body.name || !body.email) { err.textContent = 'Name and email are required.'; return; }
          if (!picked) { err.textContent = 'Choose a date.'; return; }
          if (!time) { err.textContent = 'Choose a time.'; return; }
          const r = await api('/api/datepicker/book', body);
          if (!r.ok) { err.textContent = r.data.error; return; }
          err.textContent = ''; const d = document.getElementById('done'); d.hidden = false; d.textContent = `Booked! Confirmation ${r.data.ref}.`;
        };
      },
    }));
  },
  api(method, sub, body, { state, url }) {
    if (sub === 'slots') { const d = url.searchParams.get('date') || ''; return json({ slots: /^\d{4}-\d\d-\d\d$/.test(d) ? slotsFor(d) : [] }); }
    if (sub === 'book') {
      if (!slotsFor(String(body.date)).includes(body.time)) return json({ error: 'That time is no longer available.' }, 409);
      state.bookings.push(body); return json({ ref: 'NS-' + (7700 + state.bookings.length) });
    }
    return null;
  },
  check(s) {
    if (s.bookings.length !== 1) return { pass: false, detail: `${s.bookings.length} bookings` };
    const b = s.bookings[0];
    const pass = b.date === '2026-05-14' && b.time === '15:30' && b.name === 'Ava Patel' && String(b.email).toLowerCase() === 'ava.patel@example.com';
    return { pass, detail: `booking ${b.date} ${b.time} ${b.name} ${b.email}` };
  },
};

// 7. iframe-form -----------------------------------------------------------
T['iframe-form'] = {
  init: () => ({ billing: { email: 'billing@northwind.example', company: 'Northwind Traders', vat: '', country: 'GB' }, billingSaves: 0, profileSaves: [] }),
  page(sub, { state }) {
    if (sub === 'frame') {
      const b = state.billing;
      return html(`<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Billing details</title><style>${BASE_CSS}body{background:#fff}</style></head><body><div style="padding:16px">
<h2>Billing details</h2><p class="hint">Managed by Paywell. Invoices are sent to the billing email.</p>
<form id="f"><label class="field" for="bemail">Billing email</label><input id="bemail" type="email" value="${esc(b.email)}">
<label class="field" for="company">Company name</label><input id="company" value="${esc(b.company)}">
<div class="row"><div><label class="field" for="vat">VAT ID</label><input id="vat" value="${esc(b.vat)}"></div>
<div><label class="field" for="bcountry">Country</label><select id="bcountry">${[['GB', 'United Kingdom'], ['IE', 'Ireland'], ['DE', 'Germany'], ['FR', 'France']].map(([c, n]) => `<option value="${c}"${c === b.country ? ' selected' : ''}>${n}</option>`).join('')}</select></div></div>
<div id="msg" role="status" class="ok"></div><div class="actions"><button class="primary" type="submit">Save billing details</button></div></form></div>
<script>(${commonClient})();
document.getElementById('f').onsubmit=async(e)=>{e.preventDefault();const g=(id)=>document.getElementById(id).value.trim();
await api('/api/iframe-form/billing',{email:g('bemail'),company:g('company'),vat:g('vat'),country:g('bcountry')});document.getElementById('msg').textContent='Billing details saved.';};</script></body></html>`);
    }
    return html(page({
      title: 'Account settings - Northwind', app: 'Northwind Portal', nav: [['Overview', '#'], ['Account', '#', true]], user: 'NT',
      body: `<main><h1>Account settings</h1><p class="sub">Manage your profile and billing.</p>
<section class="card" aria-labelledby="ph"><h2 id="ph">Profile</h2>
<div class="row"><div><label class="field" for="fullname">Full name</label><input id="fullname" value="Nadia Torres"></div><div><label class="field" for="pemail">Email</label><input id="pemail" type="email" value="nadia@northwind.example"></div></div>
<div class="actions"><button id="saveProfile">Save profile</button></div></section>
<section class="card" aria-labelledby="bh"><h2 id="bh">Billing</h2>
<iframe src="/t/iframe-form/frame" title="Billing details" style="width:100%;height:380px;border:1px solid var(--border);border-radius:8px"></iframe></section></main>`,
      script() {
        document.getElementById('saveProfile').onclick = async () => { await api('/api/iframe-form/profile', { name: document.getElementById('fullname').value, email: document.getElementById('pemail').value }); toast('Profile saved'); };
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub === 'billing') { state.billing = body; state.billingSaves++; return json({ ok: true }); }
    if (sub === 'profile') { state.profileSaves.push(body); return json({ ok: true }); }
    return null;
  },
  check(s) {
    const b = s.billing;
    const pass = s.billingSaves > 0 && b.email.toLowerCase() === 'ap@northwind.example' && b.vat.replace(/\s/g, '') === 'GB123456789' && b.company === 'Northwind Traders' && b.country === 'GB' && !s.profileSaves.some((p) => p.email !== 'nadia@northwind.example');
    return { pass, detail: `billing saves=${s.billingSaves} ${JSON.stringify(b)} profileSaves=${s.profileSaves.length}` };
  },
};

// 8. shadow-dom ------------------------------------------------------------
T['shadow-dom'] = {
  init: () => ({ redeemed: [], promoTries: [] }),
  page() {
    return html(page({
      title: 'Your bag - Fernway', app: 'Fernway', nav: [['Shop', '#'], ['Bag (2)', '#', true]], user: 'LS',
      body: `<main><h1>Your bag</h1><p class="sub">Free shipping on orders over $75.</p>
<div class="card"><p>Linen overshirt, sage, M <span style="float:right">$88.00</span></p><p>Canvas tote <span style="float:right">$24.00</span></p><hr><p><b>Total</b> <span style="float:right" id="total">$112.00</span></p></div>
<div class="card"><h2>Discount code</h2><div class="row" style="align-items:end"><div><label class="field" for="promo">Promo code</label><input id="promo"></div><div style="flex:0"><button id="applyPromo">Apply</button></div></div><div id="promoMsg" class="err" role="status"></div></div>
<div class="card"><h2>Gift card</h2><gift-card-redeemer></gift-card-redeemer></div>
<div class="actions"><button class="primary">Proceed to checkout</button></div></main>`,
      script() {
        const CSS = `:host{display:block;font:14px system-ui,sans-serif}label{display:block;font-weight:500;margin:0 0 4px}input{font:inherit;width:100%;padding:8px 10px;border:1px solid #cfd4dc;border-radius:8px;box-sizing:border-box}button{font:inherit;padding:8px 14px;border-radius:8px;border:1px solid #4f46e5;background:#4f46e5;color:#fff;cursor:pointer}`;
        customElements.define('ui-text-field', class extends HTMLElement {
          constructor() { super(); const r = this.attachShadow({ mode: 'open' }); r.innerHTML = `<style>${CSS}</style><label for="i"></label><input id="i" autocomplete="off">`; }
          connectedCallback() { this.shadowRoot.querySelector('label').textContent = this.getAttribute('label'); this.shadowRoot.querySelector('input').placeholder = this.getAttribute('placeholder') || ''; }
          get value() { return this.shadowRoot.querySelector('input').value; }
        });
        customElements.define('ui-button', class extends HTMLElement {
          constructor() { super(); const r = this.attachShadow({ mode: 'open' }); r.innerHTML = `<style>${CSS}</style><button type="button"><slot></slot></button>`; }
        });
        customElements.define('gift-card-redeemer', class extends HTMLElement {
          constructor() {
            super(); const r = this.attachShadow({ mode: 'open' });
            r.innerHTML = `<style>${CSS}.row{display:flex;gap:10px;align-items:end}.row ui-text-field{flex:1}.msg{margin-top:8px;color:#15803d}.msg.bad{color:#dc2626}</style>
<p style="margin:0 0 10px;color:#667085">Have a Fernway gift card? Enter the 16 character code from the back of the card.</p>
<div class="row"><ui-text-field label="Gift card code" placeholder="GIFT-XXXX-XXXX"></ui-text-field><ui-button>Redeem</ui-button></div><div class="msg" role="status"></div>`;
            r.querySelector('ui-button').addEventListener('click', async () => {
              const code = r.querySelector('ui-text-field').value.trim();
              const res = await api('/api/shadow-dom/redeem', { code });
              const m = r.querySelector('.msg');
              m.className = 'msg' + (res.ok ? '' : ' bad');
              m.textContent = res.ok ? `Gift card applied: $${res.data.amount}.00 credited.` : res.data.error;
              if (res.ok) document.getElementById('total').textContent = '$' + (112 - res.data.amount).toFixed(2);
            });
          }
        });
        document.getElementById('applyPromo').onclick = async () => {
          const r = await api('/api/shadow-dom/promo', { code: document.getElementById('promo').value.trim() });
          document.getElementById('promoMsg').textContent = r.data.error;
        };
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub === 'promo') { state.promoTries.push(body.code); return json({ error: 'This promo code is not valid.' }, 400); }
    if (sub === 'redeem') {
      const code = String(body.code || '').toUpperCase();
      if (code !== 'GIFT-7Q2M-K9XD') return json({ error: 'We could not find a gift card with that code.' }, 400);
      if (!state.redeemed.includes(code)) state.redeemed.push(code);
      return json({ amount: 50 });
    }
    return null;
  },
  check: (s) => ({ pass: s.redeemed.includes('GIFT-7Q2M-K9XD'), detail: `redeemed=${JSON.stringify(s.redeemed)} promoTries=${JSON.stringify(s.promoTries)}` }),
};

// 9. table-pagination (answer) ---------------------------------------------
const EMPLOYEES = (() => {
  const rng = mulberry32(20260310);
  const first = ['Ada', 'Ben', 'Chloe', 'Dmitri', 'Elena', 'Farah', 'Gus', 'Hana', 'Ivan', 'Jade', 'Kofi', 'Lena', 'Marco', 'Nina', 'Omar', 'Pia', 'Quinn', 'Rosa', 'Sven', 'Tara', 'Umar', 'Vera', 'Wes', 'Xin', 'Yara', 'Zane', 'Aiko', 'Bruno', 'Cora', 'Dev', 'Esme', 'Felix'];
  const last = ['Abbott', 'Bauer', 'Castillo', 'Dvorak', 'Eriksen', 'Fischer', 'Garcia', 'Hughes', 'Ito', 'Jensen', 'Kowalski', 'Lindqvist', 'Moreau', 'Nakamura', 'Okafor', 'Petrov', 'Quiroga', 'Rossi', 'Schmidt', 'Tanaka', 'Usman', 'Varga', 'Wright', 'Yilmaz', 'Zhou'];
  const depts = ['Engineering', 'Finance', 'Marketing', 'Sales', 'Support', 'Design', 'Legal', 'Operations'];
  const locs = ['Berlin', 'Austin', 'Toronto', 'Lisbon', 'Singapore', 'Remote'];
  const used = new Set(), names = new Set(), out = [];
  for (let i = 0; i < 64; i++) {
    let name; do { name = first[Math.floor(rng() * first.length)] + ' ' + last[Math.floor(rng() * last.length)]; } while (names.has(name));
    names.add(name);
    let sal = 52000 + Math.floor(rng() * 1300) * 100; while (used.has(sal)) sal += 100; used.add(sal);
    const y = 2015 + Math.floor(rng() * 10), m = 1 + Math.floor(rng() * 12), d = 1 + Math.floor(rng() * 28);
    out.push({ id: 'E-' + (1001 + i), name, dept: depts[Math.floor(rng() * depts.length)], loc: locs[Math.floor(rng() * locs.length)], salary: sal, start: `${y}-${String(m).padStart(2, '0')}-${String(d).padStart(2, '0')}` });
  }
  return out;
})();
T['table-pagination'] = {
  init: () => ({}),
  page() {
    return html(page({
      title: 'People directory - Orbit HR', app: 'Orbit HR', nav: [['People', '#', true], ['Time off', '#'], ['Reports', '#']], user: 'HR',
      css: `th button{all:unset;cursor:pointer;display:inline-flex;gap:4px;align-items:center}th button:focus-visible{outline:2px solid #a5b4fc}.pager{display:flex;gap:6px;align-items:center;justify-content:flex-end;margin-top:12px}.pager button[aria-current]{background:var(--accent);color:#fff;border-color:var(--accent)}td.num{font-variant-numeric:tabular-nums}`,
      body: `<main><h1>People directory</h1><p class="sub">64 employees across 8 departments.</p>
<div class="card" style="padding:0;overflow:hidden"><table class="data" aria-label="Employees"><thead><tr id="hdr"></tr></thead><tbody id="rows"></tbody></table></div>
<nav class="pager" aria-label="Pagination" id="pager"></nav></main>`,
      data: EMPLOYEES,
      script(emps) {
        const COLS = [['id', 'ID'], ['name', 'Name'], ['dept', 'Department'], ['loc', 'Location'], ['start', 'Start date'], ['salary', 'Salary']];
        const PER = 10; let sort = { key: 'name', dir: 1 }, pageNo = 1;
        const fmt = (n) => '$' + n.toLocaleString('en-US');
        const render = () => {
          const rows = emps.slice().sort((a, b) => (a[sort.key] < b[sort.key] ? -1 : a[sort.key] > b[sort.key] ? 1 : 0) * sort.dir);
          const pages = Math.ceil(rows.length / PER);
          document.getElementById('hdr').innerHTML = COLS.map(([k, l]) => `<th scope="col" aria-sort="${sort.key === k ? (sort.dir > 0 ? 'ascending' : 'descending') : 'none'}"><button data-k="${k}">${l}<span aria-hidden="true">${sort.key === k ? (sort.dir > 0 ? '▲' : '▼') : '↕'}</span></button></th>`).join('');
          document.getElementById('rows').innerHTML = rows.slice((pageNo - 1) * PER, pageNo * PER).map((e) => `<tr><td>${e.id}</td><td>${h(e.name)}</td><td>${e.dept}</td><td>${e.loc}</td><td>${e.start}</td><td class="num">${fmt(e.salary)}</td></tr>`).join('');
          let p = `<button data-p="${pageNo - 1}" ${pageNo === 1 ? 'disabled' : ''}>Previous</button>`;
          for (let i = 1; i <= pages; i++) p += `<button data-p="${i}" aria-label="Page ${i}"${i === pageNo ? ' aria-current="page"' : ''}>${i}</button>`;
          p += `<button data-p="${pageNo + 1}" ${pageNo === pages ? 'disabled' : ''}>Next</button><span class="hint">Showing ${(pageNo - 1) * PER + 1} to ${Math.min(pageNo * PER, rows.length)} of ${rows.length}</span>`;
          document.getElementById('pager').innerHTML = p;
          document.querySelectorAll('#hdr button').forEach((b) => (b.onclick = () => { const k = b.dataset.k; sort = sort.key === k ? { key: k, dir: -sort.dir } : { key: k, dir: 1 }; pageNo = 1; render(); }));
          document.querySelectorAll('#pager button[data-p]').forEach((b) => (b.onclick = () => { pageNo = +b.dataset.p; render(); }));
        };
        render();
      },
    }));
  },
  api: () => null,
  check: () => ({ pass: false, detail: 'answer task: judged from the agent reply' }),
};

// 10. product-filter -------------------------------------------------------
const PRODUCTS = [
  // id, name, category, price, rating, wireless, anc
  ['p01', 'Pulse ANC 700', 'Headphones', 149, 4.6, true, true],
  ['p02', 'Halo Buds ANC', 'Earbuds', 99, 4.7, true, true],
  ['p03', 'Boom Cube', 'Speakers', 49, 4.3, true, false],
  ['p04', 'Studio Wire Pro', 'Headphones', 99, 4.8, false, true],
  ['p05', 'Keystone 75', 'Keyboards', 129, 4.6, true, false],
  ['p06', 'AirMotion X', 'Headphones', 129, 4.4, true, true],
  ['p07', 'Glide Mouse', 'Mice', 39, 4.5, true, false],
  ['p08', 'Pulse Lite', 'Headphones', 59, 4.6, true, false],
  ['p09', 'Vertex ANC', 'Headphones', 124, 4.9, true, true],
  ['p10', 'Riff Mini', 'Speakers', 35, 4.1, true, false],
  ['p11', 'Echo Studio Monitor', 'Headphones', 139, 4.5, false, false],
  ['p12', 'BassBox Over-Ear', 'Headphones', 79, 4.2, true, true],
  ['p13', 'Tactile TKL', 'Keyboards', 89, 4.4, false, false],
  ['p14', 'Drift ANC', 'Headphones', 115, 4.4, true, true],
  ['p15', 'Nimbus Wireless', 'Headphones', 109, 4.5, true, false],
  ['p16', 'Orbit Earbuds', 'Earbuds', 69, 4.2, true, false],
  ['p17', 'Quietude 2 Max', 'Headphones', 199, 4.7, true, true],
  ['p18', 'Precision Pro Mouse', 'Mice', 79, 4.8, true, false],
  ['p19', 'Quietude 2', 'Headphones', 119, 4.5, true, true],
  ['p20', 'Bar Soundline', 'Speakers', 179, 4.6, true, false],
  ['p21', 'Halo Buds Lite', 'Earbuds', 49, 4.0, true, false],
  ['p22', 'Lumen Keys', 'Keyboards', 59, 4.2, true, false],
  ['p23', 'Arc ANC Studio', 'Headphones', 159, 4.6, true, true],
  ['p24', 'Trail Speaker', 'Speakers', 89, 4.4, true, false],
  ['p25', 'Commuter ANC', 'Headphones', 99, 4.3, true, true],
  ['p26', 'Ergo Vertical Mouse', 'Mice', 49, 4.3, false, false],
  ['p27', 'Calm One', 'Headphones', 135, 4.5, true, true],
  ['p28', 'Studio Wire', 'Headphones', 69, 4.6, false, false],
].map(([id, name, cat, price, rating, wireless, anc]) => ({ id, name, cat, price, rating, wireless, anc }));
T['product-filter'] = {
  init: () => ({ cart: [] }),
  page() {
    return html(page({
      title: 'Audio and accessories - Soundry', app: 'Soundry', nav: [['Shop', '#', true], ['Deals', '#'], ['Support', '#']], user: 'KB',
      css: `.layout{display:grid;grid-template-columns:200px 1fr 230px;gap:16px}.filters fieldset{border:0;padding:0;margin:0 0 14px}.filters legend{font-weight:600;margin-bottom:4px}.filters label{display:block}
.grid{display:grid;grid-template-columns:repeat(3,1fr);gap:12px}.prod{background:#fff;border:1px solid var(--border);border-radius:12px;padding:14px;display:flex;flex-direction:column;gap:4px}.prod .img{height:70px;border-radius:8px;background:linear-gradient(135deg,#e0e7ff,#f1f5f9)}.prod h3{font-size:14px;margin:6px 0 0}.prod .price{font-weight:700}.prod button{margin-top:auto}
.toolbar{display:flex;justify-content:space-between;align-items:center;margin-bottom:12px}.toolbar select{width:auto}`,
      body: `<main style="max-width:1180px"><h1>Audio and accessories</h1><p class="sub">Free returns within 30 days.</p>
<div class="layout"><aside class="filters card" aria-label="Filters">
<fieldset><legend>Category</legend>${['Headphones', 'Earbuds', 'Speakers', 'Keyboards', 'Mice'].map((c) => `<label><input type="checkbox" name="cat" value="${c}"> ${c}</label>`).join('')}</fieldset>
<fieldset><legend>Features</legend><label><input type="checkbox" id="fw"> Wireless</label><label><input type="checkbox" id="fa"> Noise cancelling</label></fieldset>
<fieldset><legend>Rating</legend><select id="fr" aria-label="Minimum rating"><option value="0">Any rating</option><option value="4">4 stars and up</option><option value="4.5">4.5 stars and up</option></select></fieldset>
</aside>
<section aria-label="Products"><div class="toolbar"><span id="count" class="hint"></span><label>Sort by <select id="sort"><option value="featured">Featured</option><option value="price-asc">Price: low to high</option><option value="price-desc">Price: high to low</option><option value="rating">Customer rating</option></select></label></div>
<div class="grid" id="grid"></div><div style="text-align:center;margin-top:12px"><button id="more">Show more</button></div></section>
<aside class="card" aria-labelledby="cartH"><h2 id="cartH">Cart</h2><div id="cart"></div></aside></div></main>`,
      data: PRODUCTS,
      script(products) {
        let shown = 9;
        const byId = Object.fromEntries(products.map((p) => [p.id, p]));
        const render = () => {
          const cats = [...document.querySelectorAll('input[name=cat]:checked')].map((x) => x.value);
          const w = document.getElementById('fw').checked, a = document.getElementById('fa').checked, r = +document.getElementById('fr').value, s = document.getElementById('sort').value;
          let list = products.filter((p) => (!cats.length || cats.includes(p.cat)) && (!w || p.wireless) && (!a || p.anc) && p.rating >= r);
          if (s === 'price-asc') list.sort((x, y) => x.price - y.price); else if (s === 'price-desc') list.sort((x, y) => y.price - x.price); else if (s === 'rating') list.sort((x, y) => y.rating - x.rating);
          document.getElementById('count').textContent = `${list.length} products`;
          document.getElementById('grid').innerHTML = list.slice(0, shown).map((p) => `<article class="prod"><div class="img" aria-hidden="true"></div><h3>${h(p.name)}</h3><div class="hint">${p.cat}</div><div>${p.rating.toFixed(1)} ★</div><div>${p.wireless ? '<span class="badge blue">Wireless</span> ' : '<span class="badge">Wired</span> '}${p.anc ? '<span class="badge green">ANC</span>' : ''}</div><div class="price">$${p.price}.00</div><button data-add="${p.id}">Add to cart</button></article>`).join('');
          document.getElementById('more').hidden = list.length <= shown;
          document.querySelectorAll('[data-add]').forEach((b) => (b.onclick = async () => { const r = await api('/api/product-filter/cart/add', { id: b.dataset.add }); drawCart(r.data.cart); toast('Added to cart'); }));
        };
        const drawCart = (cart) => {
          const el = document.getElementById('cart');
          if (!cart.length) { el.innerHTML = '<p class="hint">Your cart is empty.</p>'; return; }
          el.innerHTML = '<ul style="padding:0;list-style:none">' + cart.map((c) => `<li style="margin-bottom:8px">${h(byId[c.id].name)} × ${c.qty}<br><span class="hint">$${byId[c.id].price * c.qty}.00</span> <button class="icon" style="padding:2px 6px;line-height:1" data-rm="${c.id}">Remove</button></li>`).join('') + '</ul>';
          el.querySelectorAll('[data-rm]').forEach((b) => (b.onclick = async () => { const r = await api('/api/product-filter/cart/remove', { id: b.dataset.rm }); drawCart(r.data.cart); }));
        };
        document.querySelectorAll('.filters input, .filters select, #sort').forEach((x) => x.addEventListener('change', () => { shown = 9; render(); }));
        document.getElementById('more').onclick = () => { shown += 9; render(); };
        render(); api('/api/product-filter/cart').then((r) => drawCart(r.data.cart));
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub === 'cart' && method === 'GET') return json({ cart: state.cart });
    if (sub === 'cart/add') { const it = state.cart.find((c) => c.id === body.id); if (it) it.qty++; else if (PRODUCTS.some((p) => p.id === body.id)) state.cart.push({ id: body.id, qty: 1 }); return json({ cart: state.cart }); }
    if (sub === 'cart/remove') { state.cart = state.cart.filter((c) => c.id !== body.id); return json({ cart: state.cart }); }
    return null;
  },
  check: (s) => ({ pass: s.cart.length === 1 && s.cart[0].id === 'p19' && s.cart[0].qty === 1, detail: 'cart=' + JSON.stringify(s.cart) }),
};

// 11. infinite-scroll ------------------------------------------------------
const FEED = (() => {
  const rng = mulberry32(777);
  const authors = [['Leo Martins', 'leom'], ['Sofia Hart', 'sofhart'], ['Kenji Mori', 'kenji'], ['Amara Obi', 'amaraobi'], ['Theo Lang', 'theol'], ['Mila Novak', 'milan'], ['Ravi Shah', 'ravishah'], ['Ines Duarte', 'inesd']];
  const texts = ['Shipped a new version of our design tokens today. Feedback welcome!', 'Morning ride along the river. 42 km and one very confused heron.', 'Hot take: most meetings should be documents.', 'Finished reading a great book on urban planning. Recommendations for the next one?', 'Our team is hiring two backend engineers. DM me if curious.', 'Sourdough attempt number nine. Getting closer.', 'Conference talk slides are up on my site.', 'Rainy weekend, perfect for refactoring that old side project.', 'Trying out a new keyboard layout. Typing at half speed now.', 'Grateful for the folks who answered my question about Postgres indexes.', 'Just adopted a very loud cat named Biscuit.', 'Planning a hike next month, any trail tips for the coast?', 'New blog post: what we learned migrating 3 million rows.', 'The farmers market had the best tomatoes this week.'];
  const posts = [];
  for (let i = 0; i < 80; i++) {
    const [name, handle] = authors[Math.floor(rng() * authors.length)];
    posts.push({ id: 'post-' + (500 + i), name, handle, text: texts[Math.floor(rng() * texts.length)], likes: Math.floor(rng() * 90), hours: i + 1 });
  }
  posts[3] = { ...posts[3], name: 'Priya Raman', handle: 'priyamakes', text: 'Glaze experiments from this week. The speckled blue finally worked!' };
  posts[26] = { ...posts[26], name: 'Priya Raman', handle: 'priyamakes', text: 'Looking for recommendations on a kiln repair technician near Oakland.' };
  posts[44] = { ...posts[44], name: 'Priya Raman', handle: 'priyamakes', text: 'Big news: Clay & Kiln, my pottery studio, opens its doors this Saturday at 10am. Come say hi!' };
  posts[61] = { ...posts[61], name: 'Priya Raman', handle: 'priyamakes', text: 'Thank you all for coming to the studio opening last weekend. Photos soon.' };
  return posts;
})();
T['infinite-scroll'] = {
  init: () => ({ liked: [] }),
  page() {
    return html(page({
      title: 'Home - Chirpline', app: 'Chirpline', nav: [['Home', '#', true], ['Explore', '#'], ['Notifications', '#']], user: 'YO',
      css: `.post{display:flex;gap:12px}.post .avatar{flex:none}.post .meta{color:var(--muted)}.post p{margin:4px 0 8px}.like[aria-pressed=true]{color:#e11d48;border-color:#fecdd3;background:#fff1f2}.like{display:inline-flex;gap:6px;align-items:center;padding:4px 10px}`,
      body: `<main style="max-width:620px"><h1>Home</h1><p class="sub">Latest posts from people you follow.</p><div id="feed" role="feed" aria-busy="false" aria-label="Posts"></div><div id="sentinel" class="hint" style="text-align:center;padding:20px">Loading more…</div></main>`,
      script() {
        let next = 1, loading = false, done = false;
        const feed = document.getElementById('feed'), sentinel = document.getElementById('sentinel');
        const load = async () => {
          if (loading || done) return; loading = true; feed.setAttribute('aria-busy', 'true');
          const r = await api('/api/infinite-scroll/feed?page=' + next);
          await new Promise((res) => setTimeout(res, 400));
          for (const p of r.data.posts) {
            const a = document.createElement('article'); a.className = 'card post'; a.setAttribute('aria-labelledby', p.id + '-n');
            a.innerHTML = `<span class="avatar" aria-hidden="true">${p.name.split(' ').map((x) => x[0]).join('')}</span><div style="flex:1"><div><b id="${p.id}-n">${h(p.name)}</b> <span class="meta">@${p.handle} · ${p.hours}h</span></div><p>${h(p.text)}</p>
<button class="like" aria-pressed="${p.liked}" data-id="${p.id}">${'<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1-1.1a5.5 5.5 0 0 0-7.8 7.8L12 21l8.8-8.6a5.5 5.5 0 0 0 0-7.8Z"/></svg>'}Like <span class="n">${p.likes}</span></button></div>`;
            feed.appendChild(a);
            a.querySelector('.like').onclick = async (e) => {
              const b = e.currentTarget; const on = b.getAttribute('aria-pressed') !== 'true';
              const res = await api('/api/infinite-scroll/like', { id: p.id, on });
              b.setAttribute('aria-pressed', String(on)); b.querySelector('.n').textContent = res.data.likes;
            };
          }
          next++; done = !r.data.more; loading = false; feed.setAttribute('aria-busy', 'false');
          if (done) sentinel.textContent = "You're all caught up.";
          check();
        };
        const check = () => { if (!done && sentinel.getBoundingClientRect().top < innerHeight + 200) load(); };
        new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && load(), { rootMargin: '200px' }).observe(sentinel);
        addEventListener('scroll', check, { passive: true });
        load();
      },
    }));
  },
  api(method, sub, body, { state, url }) {
    if (sub === 'feed') {
      const p = Math.max(1, +url.searchParams.get('page') || 1);
      const posts = FEED.slice((p - 1) * 10, p * 10).map((x) => ({ ...x, liked: state.liked.includes(x.id), likes: x.likes + (state.liked.includes(x.id) ? 1 : 0) }));
      return json({ posts, more: p * 10 < FEED.length });
    }
    if (sub === 'like') {
      state.liked = state.liked.filter((x) => x !== body.id); if (body.on) state.liked.push(body.id);
      const post = FEED.find((x) => x.id === body.id);
      return json({ likes: (post?.likes || 0) + (body.on ? 1 : 0) });
    }
    return null;
  },
  check: (s) => ({ pass: s.liked.length === 1 && s.liked[0] === FEED[44].id, detail: 'liked=' + JSON.stringify(s.liked) }),
};

// 12. kanban ---------------------------------------------------------------
const KANBAN_COLS = ['Backlog', 'To do', 'In progress', 'Review', 'Done'];
const KANBAN_INIT = [
  ['k1', 'Audit S3 bucket policies', 'Backlog', 'Security'], ['k2', 'Migrate billing cron jobs', 'Backlog', 'Billing'], ['k3', 'Dark mode for settings', 'Backlog', 'UI'],
  ['k4', 'Update onboarding emails', 'To do', 'Growth'], ['k5', 'Rate limit public API', 'To do', 'Platform'],
  ['k6', 'Migrate billing webhooks', 'In progress', 'Billing'], ['k7', 'Flaky checkout test', 'In progress', 'QA'],
  ['k8', 'Update onboarding copy', 'Review', 'Growth'], ['k9', 'Upgrade Node to 22', 'Review', 'Platform'],
  ['k10', 'SSO for enterprise', 'Done', 'Security'], ['k11', 'CSV export for invoices', 'Done', 'Billing'],
];
T['kanban'] = {
  init: () => ({ cards: KANBAN_INIT.map(([id, title, col, tag]) => ({ id, title, col, tag, archived: false })) }),
  page() {
    return html(page({
      title: 'Sprint 14 - Trackwise', app: 'Trackwise', nav: [['Board', '#', true], ['Backlog', '#'], ['Reports', '#']], user: 'DM',
      css: `.board{display:grid;grid-template-columns:repeat(5,1fr);gap:12px;align-items:start}.col{background:#eceff3;border-radius:12px;padding:10px}.col h2{font-size:13px;text-transform:uppercase;color:var(--muted);display:flex;justify-content:space-between}
.kcard{background:#fff;border-radius:10px;padding:10px;margin-bottom:8px;box-shadow:0 1px 2px rgba(0,0,0,.06);position:relative}.kcard h3{font-size:14px;margin:0 22px 6px 0;font-weight:500}.kcard .more{position:absolute;top:6px;right:4px}
[role=menu]{position:absolute;z-index:20;background:#fff;border:1px solid var(--border);border-radius:10px;padding:4px;min-width:160px;box-shadow:0 10px 30px rgba(0,0,0,.15);list-style:none;margin:0}[role=menu] [role=menuitem]{padding:6px 10px;border-radius:6px;cursor:pointer;display:block}[role=menu] [role=menuitem]:hover,[role=menu] [role=menuitem]:focus{background:var(--accent-soft);outline:none}`,
      body: `<main style="max-width:1200px"><h1>Sprint 14</h1><p class="sub">Mar 2 to Mar 13 · 11 issues</p><div class="board" id="board"></div></main>`,
      data: KANBAN_COLS,
      script(cols) {
        let cards = [];
        const closeMenus = () => { document.querySelectorAll('[role=menu]').forEach((m) => m.remove()); document.querySelectorAll('.more[aria-expanded=true]').forEach((b) => b.setAttribute('aria-expanded', 'false')); };
        const render = () => {
          document.getElementById('board').innerHTML = cols.map((c) => {
            const cs = cards.filter((k) => k.col === c && !k.archived);
            return `<section class="col" aria-label="${c}"><h2>${c}<span>${cs.length}</span></h2>${cs.map((k) => `<div class="kcard" data-id="${k.id}"><h3>${h(k.title)}</h3><span class="badge">${k.tag}</span><button class="icon more" aria-label="Card actions" aria-haspopup="menu" aria-expanded="false">${'<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><circle cx="5" cy="12" r="2"/><circle cx="12" cy="12" r="2"/><circle cx="19" cy="12" r="2"/></svg>'}</button></div>`).join('')}</section>`;
          }).join('');
          document.querySelectorAll('.more').forEach((b) => (b.onclick = (e) => { e.stopPropagation(); const was = b.getAttribute('aria-expanded') === 'true'; closeMenus(); if (!was) openMenu(b); }));
        };
        const mkMenu = (items, anchor, label) => {
          const m = document.createElement('ul'); m.setAttribute('role', 'menu'); m.setAttribute('aria-label', label);
          m.innerHTML = items.map(([t, extra]) => `<li role="menuitem" tabindex="-1"${extra || ''}>${t}</li>`).join('');
          anchor.appendChild(m); return m;
        };
        const openMenu = (btn) => {
          const card = btn.closest('.kcard'); const k = cards.find((x) => x.id === card.dataset.id);
          btn.setAttribute('aria-expanded', 'true');
          const m = mkMenu([['Edit'], ['Move to…', ' aria-haspopup="menu" aria-expanded="false" data-move'], ['Archive', ' data-archive']], card, 'Card actions');
          m.style.right = '0'; m.style.top = '30px';
          m.querySelector('[role=menuitem]').focus();
          m.addEventListener('click', (e) => e.stopPropagation());
          const mv = m.querySelector('[data-move]');
          const openSub = () => {
            if (m.querySelector('[role=menu]')) return;
            mv.setAttribute('aria-expanded', 'true');
            const sub = mkMenu(cols.filter((c) => c !== k.col).map((c) => [c, ` data-to="${c}"`]), mv, 'Move to');
            sub.style.left = '100%'; sub.style.top = '0'; mv.style.position = 'relative';
            sub.querySelectorAll('[data-to]').forEach((it) => (it.onclick = async (e) => { e.stopPropagation(); const r = await api('/api/kanban/move', { id: k.id, to: it.dataset.to }); cards = r.data.cards; closeMenus(); render(); }));
            sub.querySelector('[role=menuitem]').focus();
          };
          mv.onclick = (e) => { if (e.target === mv) openSub(); };
          mv.onkeydown = (e) => { if (e.key === 'ArrowRight' || e.key === 'Enter') { e.preventDefault(); openSub(); } };
          m.querySelector('[data-archive]').onclick = async () => { const r = await api('/api/kanban/archive', { id: k.id }); cards = r.data.cards; closeMenus(); render(); };
          m.querySelector('[role=menuitem]').onclick = () => { closeMenus(); toast('Editing is not available in this demo.'); };
        };
        document.addEventListener('click', closeMenus);
        document.addEventListener('keydown', (e) => { if (e.key === 'Escape') closeMenus(); });
        api('/api/kanban/board').then((r) => { cards = r.data.cards; render(); });
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub === 'board') return json({ cards: state.cards });
    const c = state.cards.find((x) => x.id === body?.id);
    if (sub === 'move' && c && KANBAN_COLS.includes(body.to)) { c.col = body.to; return json({ cards: state.cards }); }
    if (sub === 'archive' && c) { c.archived = true; return json({ cards: state.cards }); }
    return null;
  },
  check(s) {
    const want = Object.fromEntries(KANBAN_INIT.map(([id, , col]) => [id, col])); want.k6 = 'Review'; want.k8 = 'Done';
    const bad = s.cards.filter((c) => c.archived || c.col !== want[c.id]).map((c) => `${c.title}: ${c.archived ? 'archived' : c.col}`);
    return { pass: !bad.length, detail: bad.length ? 'wrong: ' + bad.join('; ') : 'board correct' };
  },
};

// 13. settings-toggles -----------------------------------------------------
const SETTINGS_INIT = { email_product: true, email_digest: false, email_security: true, email_replies: true, email_mentions: true, push_mentions: false, push_replies: true, push_reminders: false };
T['settings-toggles'] = {
  init: () => ({ settings: { ...SETTINGS_INIT }, saves: 0 }),
  page(sub, { state }) {
    return html(page({
      title: 'Notification settings - Plume', app: 'Plume', nav: [['Inbox', '#'], ['Settings', '#', true]], user: 'EV',
      css: `.set{display:flex;align-items:center;justify-content:space-between;padding:12px 0;border-bottom:1px solid var(--border)}.set:last-child{border:0}.set .d{color:var(--muted);font-size:13px}
[role=switch]{width:42px;height:24px;border-radius:999px;border:0;background:#cbd5e1;position:relative;padding:0;flex:none}[role=switch]::after{content:"";position:absolute;top:3px;left:3px;width:18px;height:18px;border-radius:50%;background:#fff;transition:left .15s}[role=switch][aria-checked=true]{background:var(--accent)}[role=switch][aria-checked=true]::after{left:21px}
.savebar{position:sticky;bottom:0;background:#fff;border-top:1px solid var(--border);padding:12px 0;display:flex;justify-content:flex-end;gap:8px;align-items:center}`,
      body: `<main style="max-width:680px"><h1>Notifications</h1><p class="sub">Choose how Plume keeps you in the loop.</p>
<section class="card" aria-labelledby="eh"><h2 id="eh">Email</h2>
${[['email_product', 'Product announcements', 'New features and improvements.'], ['email_digest', 'Weekly digest', 'A summary of activity every Monday.'], ['email_security', 'Security alerts', 'Sign-ins from new devices.'], ['email_replies', 'Replies', 'When someone replies to your comment.'], ['email_mentions', 'Mentions', 'When someone @mentions you.']].map(([k, l, d]) => `<div class="set"><div><div id="${k}-l">${l}</div><div class="d">${d}</div></div><button role="switch" data-k="${k}" aria-labelledby="${k}-l"></button></div>`).join('')}</section>
<section class="card" aria-labelledby="ph"><h2 id="ph">Push notifications</h2>
${[['push_mentions', 'Mentions', 'When someone @mentions you.'], ['push_replies', 'Replies', 'When someone replies to your comment.'], ['push_reminders', 'Reminders', 'Task due date reminders.']].map(([k, l, d]) => `<div class="set"><div><div id="${k}-l">${l}</div><div class="d">${d}</div></div><button role="switch" data-k="${k}" aria-labelledby="${k}-l"></button></div>`).join('')}</section>
<div class="savebar"><span id="st" class="hint" role="status"></span><button id="reset">Discard</button><button class="primary" id="save" disabled>Save changes</button></div></main>`,
      data: state.settings,
      script(initial) {
        let base = { ...initial }, cur = { ...initial };
        const sync = () => {
          document.querySelectorAll('[role=switch]').forEach((b) => b.setAttribute('aria-checked', String(cur[b.dataset.k])));
          const dirty = JSON.stringify(base) !== JSON.stringify(cur);
          document.getElementById('save').disabled = !dirty; document.getElementById('reset').disabled = !dirty;
          document.getElementById('st').textContent = dirty ? 'You have unsaved changes.' : '';
        };
        document.querySelectorAll('[role=switch]').forEach((b) => (b.onclick = () => { cur[b.dataset.k] = !cur[b.dataset.k]; sync(); }));
        document.getElementById('reset').onclick = () => { cur = { ...base }; sync(); };
        document.getElementById('save').onclick = async () => { await api('/api/settings-toggles/save', cur); base = { ...cur }; sync(); toast('Notification settings saved'); };
        sync();
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub !== 'save') return null;
    const s = {}; for (const k of Object.keys(SETTINGS_INIT)) s[k] = !!body[k];
    state.settings = s; state.saves++; return json({ ok: true });
  },
  check(s) {
    const want = { ...SETTINGS_INIT, email_digest: true, email_product: false, push_mentions: true };
    const bad = Object.keys(want).filter((k) => s.settings[k] !== want[k]);
    return { pass: s.saves > 0 && !bad.length, detail: s.saves ? (bad.length ? 'wrong: ' + bad.map((k) => `${k}=${s.settings[k]}`).join(', ') : 'settings correct') : 'never saved' };
  },
};

// 14. delete-dialog --------------------------------------------------------
const KEYS = [['production-web', 'pk_live_…9f2a', 'Jan 4, 2026', '2 minutes ago'], ['staging-ci-legacy', 'pk_test_…11c0', 'Jun 18, 2024', '5 months ago'], ['analytics-export', 'pk_live_…7b3e', 'Nov 2, 2025', '1 day ago'], ['staging-ci', 'pk_test_…a7d4', 'Sep 9, 2025', '3 hours ago'], ['local-dev', 'pk_test_…0e81', 'Feb 11, 2026', 'yesterday']];
T['delete-dialog'] = {
  init: () => ({ revoked: [] }),
  page(sub, { state }) {
    return html(page({
      title: 'API keys - Stackhouse', app: 'Stackhouse', nav: [['Projects', '#'], ['API keys', '#', true], ['Usage', '#']], user: 'OK',
      body: `<main><h1>API keys</h1><p class="sub">Keys grant full access to your project. Revoked keys stop working immediately.</p>
<div class="card" style="padding:0;overflow:hidden"><table class="data" aria-label="API keys"><thead><tr><th scope="col">Name</th><th scope="col">Key</th><th scope="col">Created</th><th scope="col">Last used</th><th scope="col"><span class="sr-only">Actions</span></th></tr></thead><tbody id="rows"></tbody></table></div></main>`,
      data: { keys: KEYS, revoked: state.revoked },
      script({ keys, revoked }) {
        const rows = document.getElementById('rows');
        const render = () => {
          rows.innerHTML = keys.filter(([n]) => !revoked.includes(n)).map(([n, k, c, u]) => `<tr><td><b>${n}</b></td><td><code>${k}</code></td><td>${c}</td><td>${u}</td><td style="text-align:right"><button class="revoke" data-n="${n}">Revoke</button></td></tr>`).join('');
          rows.querySelectorAll('.revoke').forEach((b) => (b.onclick = () => confirmRevoke(b.dataset.n, b)));
        };
        const confirmRevoke = (name, opener) => {
          const d = document.createElement('div'); d.className = 'backdrop';
          d.innerHTML = `<div class="modal" role="alertdialog" aria-modal="true" aria-labelledby="dt" aria-describedby="dd"><h2 id="dt">Revoke API key?</h2>
<p id="dd">Any application using <b>${name}</b> will immediately lose access. This cannot be undone.</p>
<label class="field" for="conf">Type <b>${name}</b> to confirm</label><input id="conf" autocomplete="off">
<div class="actions"><button id="cancel">Cancel</button><button class="danger" id="go" disabled>Revoke key</button></div></div>`;
          document.body.appendChild(d); document.querySelector('main').inert = true;
          const inp = d.querySelector('#conf'), go = d.querySelector('#go');
          const close = () => { d.remove(); document.querySelector('main').inert = false; opener?.focus(); };
          inp.addEventListener('input', () => (go.disabled = inp.value.trim() !== name));
          d.querySelector('#cancel').onclick = close;
          d.addEventListener('keydown', (e) => { if (e.key === 'Escape') close(); });
          go.onclick = async () => { await api('/api/delete-dialog/revoke', { name }); revoked.push(name); close(); render(); toast(`Key ${name} revoked`); };
          inp.focus();
        };
        render();
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub !== 'revoke') return null;
    if (!state.revoked.includes(body.name)) state.revoked.push(body.name);
    return json({ ok: true });
  },
  check: (s) => ({ pass: s.revoked.length === 1 && s.revoked[0] === 'staging-ci', detail: 'revoked=' + JSON.stringify(s.revoked) }),
};

// 15. tabs-accordion -------------------------------------------------------
T['tabs-accordion'] = {
  init: () => ({ general: [], webhooks: [] }),
  page() {
    return html(page({
      title: 'Project settings - Deployly', app: 'Deployly', nav: [['Projects', '#', true], ['Teams', '#']], user: 'SG',
      css: `[role=tablist]{display:flex;gap:4px;border-bottom:1px solid var(--border);margin-bottom:16px}[role=tab]{border:0;border-bottom:2px solid transparent;border-radius:0;background:transparent;padding:10px 14px;color:var(--muted)}[role=tab][aria-selected=true]{color:var(--text);border-color:var(--accent);font-weight:600}
.acc{border:1px solid var(--border);border-radius:10px;margin-bottom:10px;background:#fff}.acc h3{margin:0}.acc h3 button{width:100%;text-align:left;border:0;background:transparent;padding:14px 16px;font-weight:600;display:flex;justify-content:space-between}.acc h3 button::after{content:"+"}.acc h3 button[aria-expanded=true]::after{content:"−"}.acc [role=region]{padding:0 16px 16px}`,
      body: `<main style="max-width:720px"><h1>web-frontend</h1><p class="sub">Project settings</p>
<div role="tablist" aria-label="Settings sections"><button role="tab" id="t-gen" aria-controls="p-gen" aria-selected="true">General</button><button role="tab" id="t-int" aria-controls="p-int" aria-selected="false" tabindex="-1">Integrations</button><button role="tab" id="t-adv" aria-controls="p-adv" aria-selected="false" tabindex="-1">Advanced</button></div>
<div role="tabpanel" id="p-gen" aria-labelledby="t-gen" class="card"><label class="field" for="pname">Project name</label><input id="pname" value="web-frontend">
<label class="field" for="branch">Production branch</label><input id="branch" value="main">
<label class="field" for="retention">Log retention (days)</label><input id="retention" type="number" value="7">
<div class="actions"><button class="primary" id="saveGen">Save</button></div></div>
<div role="tabpanel" id="p-int" aria-labelledby="t-int" class="card" hidden><p>Connected: GitHub (acme/web-frontend), Slack (#deploys).</p><p class="hint">Webhook delivery is configured under Advanced.</p></div>
<div role="tabpanel" id="p-adv" aria-labelledby="t-adv" hidden>
${[['cache', 'Build cache', `<p>Cache size: 1.2 GB.</p><button>Clear build cache</button>`],
  ['hooks', 'Webhooks', `<label class="field" for="hurl">Endpoint URL</label><input id="hurl" value="https://hooks.acme.dev/deployly">
<div class="row"><div><label class="field" for="retry">Retry limit</label><input id="retry" type="number" min="0" max="10" value="3"></div><div><label class="field" for="timeout">Timeout (seconds)</label><input id="timeout" type="number" min="1" max="60" value="10"></div></div>
<div class="actions"><button class="primary" id="saveHooks">Save webhook settings</button></div>`],
  ['prot', 'Deployment protection', `<label><input type="checkbox" checked> Require approval for production deploys</label>`]].map(([id, t, inner]) => `<div class="acc"><h3><button aria-expanded="false" aria-controls="a-${id}" id="b-${id}">${t}</button></h3><div role="region" id="a-${id}" aria-labelledby="b-${id}" hidden>${inner}</div></div>`).join('')}
</div></main>`,
      script() {
        const tabs = [...document.querySelectorAll('[role=tab]')];
        const sel = (t) => tabs.forEach((x) => { const on = x === t; x.setAttribute('aria-selected', String(on)); x.tabIndex = on ? 0 : -1; document.getElementById(x.getAttribute('aria-controls')).hidden = !on; });
        tabs.forEach((t, i) => { t.onclick = () => sel(t); t.onkeydown = (e) => { const d = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0; if (d) { const n = tabs[(i + d + tabs.length) % tabs.length]; sel(n); n.focus(); } }; });
        document.querySelectorAll('.acc h3 button').forEach((b) => (b.onclick = () => { const on = b.getAttribute('aria-expanded') !== 'true'; b.setAttribute('aria-expanded', String(on)); document.getElementById(b.getAttribute('aria-controls')).hidden = !on; }));
        document.getElementById('saveGen').onclick = async () => { await api('/api/tabs-accordion/general', { name: document.getElementById('pname').value, branch: document.getElementById('branch').value, retention: +document.getElementById('retention').value }); toast('Settings saved'); };
        document.getElementById('saveHooks').onclick = async () => { await api('/api/tabs-accordion/webhooks', { url: document.getElementById('hurl').value, retry: +document.getElementById('retry').value, timeout: +document.getElementById('timeout').value }); toast('Webhook settings saved'); };
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub === 'general') { state.general.push(body); return json({ ok: true }); }
    if (sub === 'webhooks') { state.webhooks.push(body); return json({ ok: true }); }
    return null;
  },
  check(s) {
    const w = s.webhooks.at(-1);
    if (!w) return { pass: false, detail: 'webhook settings never saved' };
    return { pass: w.retry === 7 && w.timeout === 25 && w.url === 'https://hooks.acme.dev/deployly', detail: 'webhooks=' + JSON.stringify(w) };
  },
};

// 16. icon-buttons ---------------------------------------------------------
const MEMBERS = [['m1', 'Avery Brooks', 'avery.brooks@acme.test', 'Owner'], ['m2', 'Jordan Lee', 'jordan.lee@acme.test', 'Editor'], ['m3', 'Sam Ortiz', 'sam.ortiz@acme.test', 'Viewer'], ['m4', 'Jordan Leigh', 'jordan.leigh@acme.test', 'Viewer'], ['m5', 'Priya Nair', 'priya.nair@acme.test', 'Editor'], ['m6', 'Chen Wei', 'chen.wei@acme.test', 'Viewer']];
T['icon-buttons'] = {
  init: () => ({ members: MEMBERS.map(([id, name, email, role]) => ({ id, name, email, role, removed: false })) }),
  page(sub, { state }) {
    return html(page({
      title: 'Members - Acme workspace', app: 'Acme Workspace', nav: [['Overview', '#'], ['Members', '#', true], ['Billing', '#']], user: 'AB',
      css: `td .who{display:flex;gap:10px;align-items:center}td .who small{display:block;color:var(--muted)}td.act{text-align:right;white-space:nowrap}td select{width:auto}`,
      body: `<main><h1>Members</h1><p class="sub">People with access to the Acme workspace.</p>
<div class="card" style="padding:0;overflow:hidden"><table class="data" aria-label="Members"><thead><tr><th scope="col">Member</th><th scope="col">Role</th><th scope="col"><span class="sr-only">Actions</span></th></tr></thead><tbody id="rows"></tbody></table></div></main>`,
      data: state.members,
      script(members) {
        const ROLES = ['Viewer', 'Editor', 'Admin', 'Owner'];
        let editing = null;
        const EDIT = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>';
        const TRASH = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M19 6l-1 14H6L5 6"/></svg>';
        const CHECK = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" aria-hidden="true"><path d="M20 6 9 17l-5-5"/></svg>';
        const X = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12"/></svg>';
        const render = () => {
          document.getElementById('rows').innerHTML = members.filter((m) => !m.removed).map((m) => `<tr data-id="${m.id}"><td><div class="who"><span class="avatar" aria-hidden="true">${m.name.split(' ').map((x) => x[0]).join('')}</span><div>${h(m.name)}<small>${m.email}</small></div></div></td>
<td>${editing === m.id ? `<select aria-label="Role">${ROLES.map((r) => `<option${r === m.role ? ' selected' : ''}>${r}</option>`).join('')}</select>` : `<span class="badge">${m.role}</span>`}</td>
<td class="act">${editing === m.id ? `<button class="icon" data-a="save" aria-label="Save">${CHECK}</button><button class="icon" data-a="cancel" aria-label="Cancel">${X}</button>` : `<button class="icon" data-a="edit" aria-label="Edit">${EDIT}</button><button class="icon" data-a="remove">${TRASH}</button>`}</td></tr>`).join('');
          document.querySelectorAll('#rows button').forEach((b) => (b.onclick = async () => {
            const tr = b.closest('tr'); const m = members.find((x) => x.id === tr.dataset.id); const a = b.dataset.a;
            if (a === 'edit') { editing = m.id; render(); tr.ownerDocument.querySelector(`tr[data-id="${m.id}"] select`).focus(); }
            else if (a === 'cancel') { editing = null; render(); }
            else if (a === 'save') { const role = tr.querySelector('select').value; await api('/api/icon-buttons/role', { id: m.id, role }); m.role = role; editing = null; render(); toast('Role updated'); }
            else if (a === 'remove') { await api('/api/icon-buttons/remove', { id: m.id }); m.removed = true; render(); toast(`${m.name} was removed`); }
          }));
        };
        render();
      },
    }));
  },
  api(method, sub, body, { state }) {
    const m = state.members.find((x) => x.id === body?.id);
    if (!m) return null;
    if (sub === 'role') { m.role = body.role; return json({ ok: true }); }
    if (sub === 'remove') { m.removed = true; return json({ ok: true }); }
    return null;
  },
  check(s) {
    const bad = s.members.filter((m) => { const want = m.id === 'm4' ? 'Admin' : MEMBERS.find((x) => x[0] === m.id)[3]; return m.removed || m.role !== want; }).map((m) => `${m.name}: ${m.removed ? 'removed' : m.role}`);
    return { pass: !bad.length, detail: bad.length ? 'wrong: ' + bad.join('; ') : 'roles correct' };
  },
};

// 17. autocomplete-search --------------------------------------------------
const PKGS = [['tiny-date', 980000, 'Minimal date parsing.'], ['tiny-date-picker', 512000, 'A small accessible date picker.'], ['tiny-dates', 450000, 'Date range helpers.'], ['tiny-date-fns-tz', 301000, 'Time zone support for tiny-date-fns.'], ['tiny-datetime', 220000, 'Format date and time strings.'], ['tiny-date-format', 150000, 'strftime style formatting.'], ['tiny-date-fn', 90000, 'Single function date utilities (unmaintained).'], ['tiny-date-fns', 61000, 'Modular date utility functions, tree shakeable.'], ['tinydate-fns', 40000, 'Fork of tiny-date-fns with legacy browser support.'], ['date-fns', 21000000, 'Modern JavaScript date utility library.'], ['tiny-emitter', 3100000, 'A tiny event emitter.'], ['tiny-invariant', 9800000, 'Tiny invariant function.'], ['tiny-glob', 1200000, 'Tiny and fast glob.'], ['tiny-warning', 4100000, 'Tiny warning function.'], ['dayjs', 18000000, 'Fast 2kB date library.'], ['luxon', 7000000, 'Immutable date wrapper.']];
const suggest = (q) => {
  q = q.trim().toLowerCase(); if (!q) return [];
  const m = PKGS.filter(([n]) => n.includes(q));
  const rank = ([n]) => (n === q ? 0 : n.startsWith(q) ? 1 : 2);
  return m.sort((a, b) => rank(a) - rank(b) || b[1] - a[1]).slice(0, 5).map(([n, d, desc]) => ({ name: n, downloads: d, desc }));
};
T['autocomplete-search'] = {
  init: () => ({ starred: [] }),
  page(sub, { state }) {
    const css = `.search{position:relative;max-width:560px}.search ul{position:absolute;left:0;right:0;top:100%;margin:4px 0 0;padding:4px;list-style:none;background:#fff;border:1px solid var(--border);border-radius:10px;box-shadow:0 10px 30px rgba(0,0,0,.12);z-index:5}.search li{padding:8px 10px;border-radius:6px;cursor:pointer}.search li small{display:block;color:var(--muted)}.search li[aria-selected=true],.search li:hover{background:var(--accent-soft)}
.star[aria-pressed=true]{background:#fef3c7;border-color:#fcd34d}`;
    const searchBox = `<div class="search" role="search"><label for="q" class="sr-only">Search packages</label><input id="q" type="text" role="combobox" aria-expanded="false" aria-controls="sugg" aria-autocomplete="list" autocomplete="off" placeholder="Search packages"><ul id="sugg" role="listbox" aria-label="Suggestions" hidden></ul></div>`;
    const nav = [['Packages', '/t/autocomplete-search/', !sub], ['Docs', '#']];
    const searchScript = function () {
      const q = document.getElementById('q'), ul = document.getElementById('sugg');
      let timer = null, items = [], active = -1, seq = 0;
      const go = (name) => (location.href = '/t/autocomplete-search/pkg/' + encodeURIComponent(name));
      const close = () => { ul.hidden = true; q.setAttribute('aria-expanded', 'false'); q.removeAttribute('aria-activedescendant'); };
      const setActive = (i) => { active = i; ul.querySelectorAll('li').forEach((li, j) => li.setAttribute('aria-selected', String(i === j))); if (i >= 0) q.setAttribute('aria-activedescendant', 'sg-' + i); };
      q.addEventListener('input', () => {
        clearTimeout(timer); close();
        timer = setTimeout(async () => {
          const my = ++seq; const r = await api('/api/autocomplete-search/suggest?q=' + encodeURIComponent(q.value));
          if (my !== seq) return;
          items = r.data.results; active = -1;
          if (!items.length) { close(); return; }
          ul.innerHTML = items.map((p, i) => `<li role="option" id="sg-${i}" aria-selected="false" data-n="${h(p.name)}">${h(p.name)}<small>${h(p.desc)} · ${p.downloads.toLocaleString('en-US')} weekly downloads</small></li>`).join('');
          ul.hidden = false; q.setAttribute('aria-expanded', 'true');
        }, 350);
      });
      q.addEventListener('keydown', (e) => {
        if (e.key === 'ArrowDown' && items.length) { e.preventDefault(); setActive(Math.min(active + 1, items.length - 1)); }
        else if (e.key === 'ArrowUp' && items.length) { e.preventDefault(); setActive(Math.max(active - 1, 0)); }
        else if (e.key === 'Enter') { e.preventDefault(); if (!ul.hidden && active >= 0) go(items[active].name); }
        else if (e.key === 'Escape') close();
      });
      q.addEventListener('blur', () => setTimeout(close, 200));
      ul.addEventListener('mousedown', (e) => e.preventDefault());
      ul.addEventListener('click', (e) => { const li = e.target.closest('li'); if (li) go(li.dataset.n); });
    };
    if (sub.startsWith('pkg/')) {
      const name = decodeURIComponent(sub.slice(4));
      const p = PKGS.find(([n]) => n === name);
      if (!p) return { status: 404, type: 'text/html', body: 'Package not found' };
      return html(page({
        title: `${name} - pkghub`, app: 'pkghub', nav, user: 'DV', css,
        body: `<main>${searchBox}<div class="card" style="margin-top:20px"><h1>${esc(name)}</h1><p>${esc(p[2])}</p><p class="hint">${p[1].toLocaleString('en-US')} weekly downloads · MIT license · v3.2.1</p>
<button class="star" id="star" aria-pressed="${state.starred.includes(name)}">★ Star</button></div></main>`,
        data: name,
        script: `function(name){(${searchScript})();const b=document.getElementById('star');b.onclick=async()=>{const on=b.getAttribute('aria-pressed')!=='true';await api('/api/autocomplete-search/star',{name,on});b.setAttribute('aria-pressed',String(on));toast(on?'Starred '+name:'Unstarred '+name);};}`,
      }));
    }
    return html(page({
      title: 'pkghub - discover packages', app: 'pkghub', nav, user: 'DV', css,
      body: `<main><h1>Find a package</h1><p class="sub">Search 2 million open source packages.</p>${searchBox}<div class="card" style="margin-top:24px"><h2>Trending this week</h2><p>dayjs · tiny-invariant · luxon · tiny-emitter</p></div></main>`,
      script: searchScript,
    }));
  },
  api(method, sub, body, { state, url }) {
    if (sub === 'suggest') return json({ results: suggest(url.searchParams.get('q') || '') });
    if (sub === 'star') { state.starred = state.starred.filter((n) => n !== body.name); if (body.on) state.starred.push(body.name); return json({ ok: true }); }
    return null;
  },
  delay: (sub) => (sub === 'suggest' ? 150 : 0),
  check: (s) => ({ pass: s.starred.length === 1 && s.starred[0] === 'tiny-date-fns', detail: 'starred=' + JSON.stringify(s.starred) }),
};

// 18. radio-cards ----------------------------------------------------------
T['radio-cards'] = {
  init: () => ({ created: [] }),
  page() {
    const group = (name, legend, opts, def) => `<fieldset class="cards"><legend>${legend}</legend><div class="opts">${opts.map(([v, t, d, extra]) => `<label class="opt"><input type="radio" class="sr-only" name="${name}" value="${v}"${v === def ? ' checked' : ''}${extra || ''}><span class="t">${t}</span><span class="d">${d}</span></label>`).join('')}</div></fieldset>`;
    return html(page({
      title: 'Create workspace - Nimbus', app: 'Nimbus', user: 'HL',
      css: `fieldset.cards{border:0;padding:0;margin:18px 0 0}fieldset.cards legend{font-weight:600;margin-bottom:8px}.opts{display:grid;grid-template-columns:repeat(3,1fr);gap:10px}
.opt{border:1.5px solid var(--border);border-radius:12px;padding:12px;cursor:pointer;display:flex;flex-direction:column;gap:2px;position:relative;background:#fff}.opt .t{font-weight:600}.opt .d{color:var(--muted);font-size:13px}
.opt:has(input:checked){border-color:var(--accent);background:var(--accent-soft)}.opt:has(input:checked)::after{content:"✓";position:absolute;top:8px;right:10px;color:var(--accent);font-weight:700}.opt:has(input:disabled){opacity:.45;cursor:not-allowed}.opt:has(input:focus-visible){outline:2px solid #a5b4fc}`,
      body: `<main style="max-width:760px"><h1>Create a workspace</h1><p class="sub">You can change these later in workspace settings.</p>
<form class="card" id="f" novalidate><label class="field" for="wname">Workspace name</label><input id="wname" name="wname">
${group('plan', 'Plan', [['starter', 'Starter', 'Free · up to 3 users'], ['growth', 'Growth', '$49 per month · 25 users'], ['scale', 'Scale', '$199 per month · unlimited']], 'starter')}
${group('billing', 'Billing cycle', [['monthly', 'Monthly', 'Pay month to month'], ['annual', 'Annual', 'Save 20%, billed yearly']], 'monthly')}
${group('region', 'Data region', [['us-east', 'US East', 'Virginia, USA'], ['eu-central', 'EU Central', 'Frankfurt, Germany'], ['ap-southeast', 'Asia Pacific', 'Singapore']], 'us-east')}
${group('support', 'Support', [['standard', 'Standard', 'Email, 2 business days'], ['priority', 'Priority', '4 hour response, chat', ' disabled']], 'standard')}
<p class="hint" id="supHint">Priority support is available on Growth and Scale plans.</p>
<div id="e" class="err" role="alert"></div><div class="actions"><button class="primary" type="submit">Create workspace</button></div></form>
<div id="done" class="card" role="status" hidden></div></main>`,
      script() {
        const f = document.getElementById('f');
        const val = (n) => f.querySelector(`input[name=${n}]:checked`)?.value;
        f.addEventListener('change', () => {
          const pri = f.querySelector('input[name=support][value=priority]');
          pri.disabled = val('plan') === 'starter';
          if (pri.disabled && pri.checked) f.querySelector('input[name=support][value=standard]').checked = true;
        });
        f.onsubmit = async (e) => {
          e.preventDefault();
          const body = { name: f.wname.value.trim(), plan: val('plan'), billing: val('billing'), region: val('region'), support: val('support') };
          if (!body.name) { document.getElementById('e').textContent = 'Workspace name is required.'; return; }
          await api('/api/radio-cards/create', body);
          f.hidden = true; const d = document.getElementById('done'); d.hidden = false; d.textContent = `Workspace "${body.name}" created.`;
        };
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub !== 'create') return null;
    state.created.push(body); return json({ ok: true });
  },
  check(s) {
    if (s.created.length !== 1) return { pass: false, detail: `${s.created.length} workspaces created` };
    const want = { name: 'Helix Labs', plan: 'growth', billing: 'annual', region: 'eu-central', support: 'priority' };
    const c = s.created[0]; const bad = Object.keys(want).filter((k) => c[k] !== want[k]);
    return { pass: !bad.length, detail: bad.length ? 'wrong: ' + bad.map((k) => `${k}=${c[k]}`).join(', ') : 'workspace correct' };
  },
};

// 19. spa-route ------------------------------------------------------------
const INVOICES = [['INV-2031', 'Initech', 1200, 'paid'], ['INV-2034', 'Globex', 860, 'open'], ['INV-2036', 'Umbrella Corp', 4300, 'open'], ['INV-2038', 'Hooli', 990, 'paid'], ['INV-2040', 'Globex', 2150, 'paid'], ['INV-2041', 'Vandelay Industries', 640, 'open'], ['INV-2043', 'Globex', 3275, 'open'], ['INV-2044', 'Initech', 410, 'open'], ['INV-2046', 'Stark Industries', 7800, 'open'], ['INV-2047', 'Wayne Enterprises', 1530, 'paid'], ['INV-2049', 'Hooli', 2210, 'open'], ['INV-2430', 'Globex', 3275, 'open']];
T['spa-route'] = {
  init: () => ({ inv: INVOICES.map(([id, customer, amount, status]) => ({ id, customer, amount, status, method: status === 'paid' ? 'card' : null })) }),
  page() {
    return html(page({
      title: 'Billing portal', app: 'Ledgerly', user: 'FN',
      css: `.shell{display:grid;grid-template-columns:190px 1fr;gap:20px}.side a{display:block;padding:8px 12px;border-radius:8px;color:var(--text);text-decoration:none}.side a[aria-current]{background:var(--accent-soft);color:var(--accent);font-weight:600}.filters{display:flex;gap:6px;margin-bottom:12px}.filters button[aria-pressed=true]{background:var(--text);color:#fff}`,
      body: `<main style="max-width:1080px"><div class="shell"><nav class="side card" aria-label="Billing"><a href="/t/spa-route/" data-link>Overview</a><a href="/t/spa-route/invoices" data-link>Invoices</a><a href="/t/spa-route/customers" data-link>Customers</a></nav><div id="view"></div></div></main>`,
      script() {
        const view = document.getElementById('view');
        const money = (n) => '$' + n.toLocaleString('en-US') + '.00';
        let filter = 'open';
        const link = () => view.querySelectorAll('a[data-link]').forEach((a) => (a.onclick = nav));
        function nav(e) { e.preventDefault(); history.pushState({}, '', this.getAttribute('href')); route(); }
        document.querySelectorAll('.side a').forEach((a) => (a.onclick = nav));
        addEventListener('popstate', route);
        async function route() {
          const p = location.pathname.replace(/^\/t\/spa-route/, '').replace(/\/$/, '') || '/';
          document.querySelectorAll('.side a').forEach((a) => { const hp = a.getAttribute('href').replace(/^\/t\/spa-route/, '').replace(/\/$/, '') || '/'; const on = hp === '/' ? p === '/' : p.startsWith(hp); on ? a.setAttribute('aria-current', 'page') : a.removeAttribute('aria-current'); });
          view.innerHTML = '<div class="card hint" aria-busy="true">Loading…</div>';
          await new Promise((r) => setTimeout(r, 500));
          if (p === '/') {
            document.title = 'Overview - Billing portal';
            view.innerHTML = `<h1>Overview</h1><p class="sub">March 2026</p><div class="card">Outstanding: <b>$23,705.00</b> across 8 open invoices.</div><div class="card"><a href="/t/spa-route/invoices" data-link>View invoices →</a></div>`;
          } else if (p === '/customers') {
            document.title = 'Customers - Billing portal';
            view.innerHTML = `<h1>Customers</h1><div class="card">Globex, Hooli, Initech, Stark Industries, Umbrella Corp, Vandelay Industries, Wayne Enterprises</div>`;
          } else if (p === '/invoices') {
            document.title = 'Invoices - Billing portal';
            const r = await api('/api/spa-route/invoices');
            const draw = () => {
              const list = r.data.invoices.filter((i) => filter === 'all' || i.status === filter);
              view.innerHTML = `<h1>Invoices</h1><p class="sub">${list.length} invoices</p><div class="filters" role="group" aria-label="Status filter">${['open', 'paid', 'all'].map((f) => `<button aria-pressed="${f === filter}" data-f="${f}">${f[0].toUpperCase() + f.slice(1)}</button>`).join('')}</div>
<div class="card" style="padding:0;overflow:hidden"><table class="data"><thead><tr><th>Invoice</th><th>Customer</th><th>Amount</th><th>Status</th></tr></thead><tbody>${list.map((i) => `<tr><td><a href="/t/spa-route/invoices/${i.id}" data-link>${i.id}</a></td><td>${i.customer}</td><td>${money(i.amount)}</td><td><span class="badge ${i.status === 'paid' ? 'green' : 'amber'}">${i.status === 'paid' ? 'Paid' : 'Open'}</span></td></tr>`).join('')}</tbody></table></div>`;
              view.querySelectorAll('[data-f]').forEach((b) => (b.onclick = () => { filter = b.dataset.f; draw(); }));
              link();
            };
            draw();
          } else if (p.startsWith('/invoices/')) {
            const id = p.split('/')[2];
            const r = await api('/api/spa-route/invoices/' + id);
            if (!r.ok) { view.innerHTML = '<div class="card">Invoice not found.</div>'; return; }
            const i = r.data.invoice; document.title = `${i.id} - Billing portal`;
            view.innerHTML = `<p><a href="/t/spa-route/invoices" data-link>← All invoices</a></p><h1>Invoice ${i.id}</h1><p class="sub">${i.customer} · ${money(i.amount)} · <span class="badge ${i.status === 'paid' ? 'green' : 'amber'}">${i.status === 'paid' ? 'Paid' : 'Open'}</span></p>
<div class="card">${i.status === 'paid' ? `<p>Paid by ${i.method.replace('_', ' ')}.</p>` : `<h2>Record a payment</h2><label class="field" for="pm">Payment method</label><select id="pm"><option value="card">Card</option><option value="bank_transfer">Bank transfer</option><option value="check">Check</option></select><div class="actions"><button class="primary" id="pay">Mark as paid</button></div>`}</div>`;
            link();
            const pay = document.getElementById('pay');
            if (pay) pay.onclick = async () => { await api('/api/spa-route/pay', { id: i.id, method: document.getElementById('pm').value }); route(); toast(`${i.id} marked as paid`); };
          } else view.innerHTML = '<div class="card">Page not found.</div>';
        }
        route();
      },
    }));
  },
  api(method, sub, body, { state }) {
    if (sub === 'invoices') return json({ invoices: state.inv });
    if (sub.startsWith('invoices/')) { const i = state.inv.find((x) => x.id === sub.slice(9)); return i ? json({ invoice: i }) : json({ error: 'not found' }, 404); }
    if (sub === 'pay') { const i = state.inv.find((x) => x.id === body.id); if (!i) return json({ error: 'not found' }, 404); i.status = 'paid'; i.method = body.method; return json({ ok: true }); }
    return null;
  },
  check(s) {
    const bad = s.inv.filter((i) => { const init = INVOICES.find((x) => x[0] === i.id); if (i.id === 'INV-2043') return !(i.status === 'paid' && i.method === 'bank_transfer'); return i.status !== init[3]; }).map((i) => `${i.id}: ${i.status}/${i.method}`);
    return { pass: !bad.length, detail: bad.length ? 'wrong: ' + bad.join('; ') : 'invoice INV-2043 paid by bank transfer' };
  },
};

// 20. split-text-answer ----------------------------------------------------
T['split-text-answer'] = {
  init: () => ({}),
  page() {
    const items = [['Basecamp 2P tent', 'TN-220', 1, 3.4, 'Reno, NV'], ['Trail stove kit', 'ST-014', 2, 1.8, 'Denver, CO'], ['Insulated sleeping pad', 'SP-090', 2, 0.6, 'Reno, NV'], ['Bear canister', 'BC-301', 1, 7.2, 'Denver, CO'], ['Titanium spork', 'UT-007', 4, 0.95, 'Denver, CO'], ['Headlamp 400lm', 'HL-400', 3, 0.15, 'Reno, NV']];
    return html(page({
      title: 'Order details - Basecamp Supply', app: 'Basecamp Supply', nav: [['Orders', '#', true], ['Returns', '#']], user: 'TQ',
      css: `.ordno span{letter-spacing:.02em}.ordno .pfx{color:var(--muted)}.ordno .hl{color:var(--accent)}`,
      body: `<main><p><a href="#">← All orders</a></p><h1>Order <span class="ordno"><span class="pfx">ORD</span><span class="pfx">-</span><span class="hl">58</span><span class="hl">3</span><span>17</span></span></h1>
<p class="sub">Placed on <span>Feb</span> <span>27</span>, <span>2026</span> · <span class="badge blue">Split shipment</span></p>
<div class="card"><p>Your order ships from <b>two warehouses</b>. Items from <em>Den</em><em>ver</em> leave first; items from Reno follow within two days.</p>
<p>Questions? Quote reference <span>ORD</span>-<span>583</span><span>17</span> when you contact us.</p></div>
<div class="card" style="padding:0;overflow:hidden"><table class="data" aria-label="Items"><thead><tr><th scope="col">Item</th><th scope="col">SKU</th><th scope="col">Qty</th><th scope="col">Unit weight</th><th scope="col">Ships from</th></tr></thead><tbody>
${items.map(([n, s, q, w, wh]) => `<tr><td>${n}</td><td><code>${s}</code></td><td>${q}</td><td>${w} kg</td><td>${wh}</td></tr>`).join('')}
</tbody></table></div></main>`,
    }));
  },
  api: () => null,
  check: () => ({ pass: false, detail: 'answer task: judged from the agent reply' }),
};

// ---------------------------------------------------------------- server

const STATE = {};
const reset = (id) => { STATE[id] = T[id].init(); };
const resetAll = () => Object.keys(T).forEach(reset);
resetAll();

async function readBody(req) {
  const chunks = []; for await (const c of req) chunks.push(c);
  const s = Buffer.concat(chunks).toString('utf8');
  if (!s) return {};
  try { return JSON.parse(s); } catch { return {}; }
}

async function handle(req, res) {
  const url = new URL(req.url, 'http://127.0.0.1');
  const p = url.pathname;
  let out = null;
  try {
    if (p === '/__health') out = json({ ok: true });
    else if (p === '/__reset' && req.method === 'POST') { resetAll(); out = json({ ok: true }); }
    else if (p.startsWith('/__reset/') && req.method === 'POST') { const id = p.slice(9); if (T[id]) { reset(id); out = json({ ok: true }); } }
    else if (p.startsWith('/__state/')) { const id = p.slice(9); if (T[id]) out = json(T[id].check(STATE[id])); }
    else if (p === '/') out = html(page({ title: 'Fixtures', app: 'Fixtures', body: `<main><h1>Benchmark fixtures</h1><ul>${Object.keys(T).map((id) => `<li><a href="/t/${id}/">${id}</a></li>`).join('')}</ul></main>` }));
    else {
      const m = p.match(/^\/(t|api)\/([a-z-]+)(?:\/(.*))?$/);
      if (m && T[m[2]]) {
        const [, kind, id, rest = ''] = m;
        const t = T[id]; const ctx = { state: STATE[id], url };
        if (kind === 't') {
          if (!rest && !p.endsWith('/')) out = redirect(p + '/');
          else out = t.page(rest.replace(/\/$/, ''), ctx);
        } else {
          const body = req.method === 'POST' ? await readBody(req) : undefined;
          const d = t.delay?.(rest) || 0; if (d) await new Promise((r) => setTimeout(r, d));
          out = t.api(req.method, rest, body, ctx);
        }
      }
    }
  } catch (e) {
    out = json({ error: String(e?.stack || e) }, 500);
  }
  if (!out) out = { status: 404, type: 'text/plain', body: 'Not found' };
  res.writeHead(out.status, { 'content-type': out.type || 'text/plain', 'cache-control': 'no-store', ...(out.headers || {}) });
  res.end(out.body);
}

export function startServer(port = 4517) {
  return new Promise((resolve) => {
    const srv = createServer(handle);
    srv.listen(port, '127.0.0.1', () => resolve(srv));
  });
}

export const TASK_IDS = Object.keys(T);

/** In-process access for harnesses that host the server: a task's verdict, and a fresh start. */
export const judge = (id) => T[id].check(STATE[id]);
export const resetTask = (id) => reset(id);

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const i = process.argv.indexOf('--port');
  const port = i > 0 ? +process.argv[i + 1] : 4517;
  await startServer(port);
  console.log(`fixtures listening on http://127.0.0.1:${port}`);
}
