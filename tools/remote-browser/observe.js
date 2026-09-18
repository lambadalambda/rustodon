// Observe, never replace, the frontend's native WebSocket transport.
// Deliberately retain no socket URL, bearer token, cookies, or request headers.
window.remoteBrowserEvents = [];
const NativeWebSocket = window.WebSocket;
window.WebSocket = class extends NativeWebSocket {
  constructor(...args) {
    super(...args);
    this.addEventListener('open', () => window.remoteBrowserEvents.push({event: 'open'}));
    this.addEventListener('message', message => {
      try {
        const frame = JSON.parse(message.data);
        if (!['update', 'status.update'].includes(frame.event)) return;
        const status = JSON.parse(frame.payload);
        if (!status.content?.includes('Remote browser 8b2f49e')) return;
        window.remoteBrowserEvents.push({event: frame.event, id: status.id,
          media: status.media_attachments.map(m => ({id: m.id, type: m.type,
            url: m.url, preview_url: m.preview_url, meta: m.meta}))});
      } catch (_) { /* Non-status frames are deliberately not retained. */ }
    });
  }
};

// Axios uses XHR: retain the actual reloaded frontend status response, not a
// diagnostic refetch or the pre-reload WebSocket representation.
window.remoteBrowserRest = [];
const nativeOpen = XMLHttpRequest.prototype.open;
XMLHttpRequest.prototype.open = function (...args) {
  this.addEventListener('load', () => {
    try {
      if (this.status !== 200 || !new URL(this.responseURL).pathname.startsWith('/api/v1/statuses/')) return;
      const status = this.responseType === 'json' ? this.response : JSON.parse(this.responseText);
      if (!status.content?.includes('Remote browser 8b2f49e')) return;
      window.remoteBrowserRest.push({id: status.id,
        media: status.media_attachments.map(m => ({id: m.id, type: m.type,
          url: m.url, preview_url: m.preview_url, meta: m.meta}))});
    } catch (_) { /* Ignore non-status responses; never retain headers/tokens. */ }
  }, {once: true});
  return nativeOpen.apply(this, args);
};
