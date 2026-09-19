// Passive observer only: no request/response modification, auth/header capture or API calls.
(() => {
  window.hashtagCalls = [];
  const open = XMLHttpRequest.prototype.open;
  const send = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function(method, url, ...rest) {
    this.hashtagRequest = {method, url: new URL(url, location.href)};
    return open.call(this, method, url, ...rest);
  };
  XMLHttpRequest.prototype.send = function(...args) {
    this.addEventListener('loadend', () => {
      const {method, url} = this.hashtagRequest;
      if (!url.pathname.startsWith('/api/')) return;
      let body;
      try { body = JSON.parse(this.responseText); } catch { body = null; }
      const tag = x => ({id:x.id, name:x.name, following:x.following, featuring:x.featuring, history:x.history, statuses_count:x.statuses_count});
      const row = {method, path:url.pathname, code:this.status};
      if (/\/tags\/|featured_tags/.test(url.pathname)) row.tags = Array.isArray(body) ? body.map(tag) : body && tag(body);
      if (url.pathname === '/api/v1/timelines/home') row.ids = Array.isArray(body) ? body.map(x=>x.id) : [];
      window.hashtagCalls.push(row);
    });
    return send.apply(this, args);
  };
})();
