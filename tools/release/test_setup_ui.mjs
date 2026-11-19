import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';

const html = readFileSync(new URL('../../crates/coppa-daemon/src/web/setup.html', import.meta.url), 'utf8');
const controller = readFileSync(new URL('../../crates/coppa-daemon/src/web/setup.js', import.meta.url), 'utf8');

class Element {
  value = '';
  hidden = false;
  textContent = '';
  children = [];
  replaceChildren() { this.children = []; }
  add(item) { this.children.push(item); }
}

const IDS = ['unlock', 'station', 'review', 'done', 'token', 'callsign', 'grid',
  'input_device_id', 'output_device_id', 'method', 'ptt-fields', 'beacon_interval_secs',
  'unlock-next', 'refresh', 'station-back', 'review-next', 'review-back', 'save',
  'summary', 'dashboard', 'conflict-dashboard', 'message'];

function loadWizard(fetch) {
  const elements = Object.fromEntries(IDS.map(id => [id, new Element()]));
  vm.runInNewContext(controller, {
    document: {getElementById: id => elements[id]},
    Option: class { constructor(name, id) { this.name = name; this.value = id; } },
    fetch,
  });
  return elements;
}

test('wizard_loads_reviews_saves_and_retries', async () => {
  for (const id of IDS.filter(id => !['ptt-fields', 'summary'].includes(id))) {
    assert.match(html, new RegExp(`id="${id}"`));
  }
  assert.match(html, /id="conflict-dashboard"[^>]*href="http:\/\/127\.0\.0\.1:8400\/"/);
  const calls = [];
  let saveAttempts = 0;
  let deviceAttempts = 0;
  const fetch = async (path, options) => {
    calls.push([path, options]);
    if (path === '/api/config' && options.method === 'PUT') {
      saveAttempts++;
      return saveAttempts === 1
        ? {ok: false, json: async () => ({error: 'audio_unavailable'})}
        : {ok: true, json: async () => ({dashboard_url: 'http://127.0.0.1:8400/'})};
    }
    if (path === '/api/devices') {
      deviceAttempts++;
      return {ok: true, json: async () => deviceAttempts === 1 ? {inputs: [], outputs: []} : {
        inputs: [{id: 'alsa:null', name: 'Null input'}],
        outputs: [{id: 'alsa:null', name: 'Null output'}]
      }};
    }
    return {ok: true, json: async () => ({})};
  };
  const elements = loadWizard(fetch);
  elements.token.value = 'secret';
  await elements['unlock-next'].onclick();
  assert.equal(elements.station.hidden, false);
  assert.match(elements.message.textContent, /Connect audio/);
  await elements.refresh.onclick();
  assert.equal(elements.input_device_id.children[1].value, 'alsa:null');
  elements.callsign.value = 'K1ABC';
  elements.grid.value = 'FN20';
  elements.input_device_id.value = 'alsa:null';
  elements.output_device_id.value = 'alsa:null';
  elements.method.value = 'none';
  elements.beacon_interval_secs.value = '0';
  elements['review-next'].onclick();
  assert.equal(elements.review.hidden, false);
  assert.match(elements.summary.textContent, /K1ABC/);
  assert.match(elements.summary.textContent, /Null input/);
  await elements.save.onclick();
  assert.equal(elements.review.hidden, false);
  assert.match(elements.message.textContent, /Could not open/);
  await elements.save.onclick();
  assert.equal(elements.done.hidden, false);
  assert.equal(elements.dashboard.href, 'http://127.0.0.1:8400/');
  const puts = calls.filter(([path, options]) => path === '/api/config' && options.method === 'PUT');
  assert.equal(puts.length, 2);
  assert.deepEqual(JSON.parse(puts[1][1].body), {
    callsign: 'K1ABC', grid: 'FN20', input_device_id: 'alsa:null',
    output_device_id: 'alsa:null', ptt: {method: 'none'}, beacon_interval_secs: 0
  });
  assert.equal(puts[1][1].headers.Authorization, 'Bearer secret');
  assert.doesNotMatch(controller, /localStorage|sessionStorage/);
});

test('save_submits_the_reviewed_draft_after_a_late_device_refresh', async () => {
  const calls = [];
  let deviceAttempts = 0;
  let releaseSecondDevices;
  const secondDevices = new Promise(resolve => { releaseSecondDevices = resolve; });
  const fetch = async (path, options = {}) => {
    calls.push([path, options]);
    if (path === '/api/devices') {
      deviceAttempts++;
      if (deviceAttempts === 1) {
        return {ok: true, json: async () => ({
          inputs: [{id: 'alsa:null', name: 'Null input'}],
          outputs: [{id: 'alsa:null', name: 'Null output'}]
        })};
      }
      const body = await secondDevices;
      return {ok: true, json: async () => body};
    }
    if (path === '/api/config' && options.method === 'PUT') {
      return {ok: true, json: async () => ({dashboard_url: 'http://127.0.0.1:8400/'})};
    }
    return {ok: true, json: async () => ({})};
  };
  const elements = loadWizard(fetch);
  elements.token.value = 'secret';
  await elements['unlock-next'].onclick();
  elements.callsign.value = 'K1ABC';
  elements.input_device_id.value = 'alsa:null';
  elements.output_device_id.value = 'alsa:null';
  elements.method.value = 'none';
  elements.beacon_interval_secs.value = '0';
  // A refresh that resolves after the operator reviewed must not change
  // what Save submits: the draft is frozen at review time.
  const pending = elements.refresh.onclick();
  elements['review-next'].onclick();
  releaseSecondDevices({
    inputs: [{id: 'alsa:other', name: 'Other input'}],
    outputs: [{id: 'alsa:other', name: 'Other output'}]
  });
  await pending;
  assert.equal(elements.input_device_id.value, '');
  await elements.save.onclick();
  assert.equal(elements.done.hidden, false);
  const puts = calls.filter(([path, options]) => path === '/api/config' && options.method === 'PUT');
  assert.equal(puts.length, 1);
  const submitted = JSON.parse(puts[0][1].body);
  assert.equal(submitted.input_device_id, 'alsa:null');
  assert.equal(submitted.output_device_id, 'alsa:null');
  assert.equal(submitted.callsign, 'K1ABC');
});

test('conflict_shows_a_dashboard_action', async () => {
  const fetch = async (path, options = {}) => {
    if (path === '/api/config' && options.method === 'PUT') {
      return {ok: false, json: async () => ({error: 'already_configured'})};
    }
    if (path === '/api/devices') {
      return {ok: true, json: async () => ({
        inputs: [{id: 'alsa:null', name: 'Null input'}],
        outputs: [{id: 'alsa:null', name: 'Null output'}]
      })};
    }
    return {ok: true, json: async () => ({})};
  };
  const elements = loadWizard(fetch);
  elements.token.value = 'secret';
  await elements['unlock-next'].onclick();
  elements.callsign.value = 'K1ABC';
  elements.input_device_id.value = 'alsa:null';
  elements.output_device_id.value = 'alsa:null';
  elements.method.value = 'none';
  elements.beacon_interval_secs.value = '0';
  elements['review-next'].onclick();
  assert.equal(elements['conflict-dashboard'].hidden, true);
  await elements.save.onclick();
  assert.equal(elements.done.hidden, true);
  assert.match(elements.message.textContent, /already complete/);
  assert.equal(elements['conflict-dashboard'].hidden, false);
});
