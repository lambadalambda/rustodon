// Passive allowlisted observation of actual frontend instance XHR completion.
window.activityCalls = [];
const open = XMLHttpRequest.prototype.open;
XMLHttpRequest.prototype.open = function(method, url, ...rest) {
  const path = new URL(url, location.href).pathname;
  if (method.toUpperCase() === 'GET' && path === '/api/v2/instance') {
    this.addEventListener('load', () => {
      try {
        const data = this.responseType === 'json' ? this.response : JSON.parse(this.responseText);
        window.activityCalls.push({path, status: this.status, active_month: data.usage.users.active_month});
      } catch (_) { window.activityCalls.push({path, status: this.status, invalid: true}); }
    });
  }
  return open.call(this, method, url, ...rest);
};
