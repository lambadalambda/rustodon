// Passive XHR observation only. Never retain headers, tokens or arbitrary bodies.
window.statusSearchEvidence = [];
const searchOpen = XMLHttpRequest.prototype.open;
XMLHttpRequest.prototype.open = function (...args) {
  this.addEventListener('load', () => {
    try {
      const u = new URL(this.responseURL);
      if (u.origin !== location.origin) return;
      const data = this.responseType === 'json' ? this.response : JSON.parse(this.responseText);
      if (u.pathname === '/api/v2/search') {
        const params = Object.fromEntries(['q', 'type', 'resolve', 'limit', 'offset'].filter(k => u.searchParams.has(k)).map(k => [k, u.searchParams.get(k)]));
        window.statusSearchEvidence.push({path: u.pathname, params, code: this.status,
          statuses: (data.statuses || []).map(s => ({id: s.id, url: s.url, uri: s.uri})),
          accounts: (data.accounts || []).length});
      } else if (/^\/api\/v1\/statuses\/\d+$/.test(u.pathname)) {
        window.statusSearchEvidence.push({path: u.pathname, code: this.status, id: data.id, url: data.url});
      }
    } catch (_) { /* Ignore non-JSON/irrelevant responses. */ }
  }, {once: true});
  return searchOpen.apply(this, args);
};
