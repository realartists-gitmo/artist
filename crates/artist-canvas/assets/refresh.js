// React Fast Refresh bootstrap.
//
// Loaded before any canvas module so the globals the transform emits —
// $RefreshReg$ and $RefreshSig$ — exist by the time the first component
// registers. Every module then overrides them with its own scoped pair and
// restores the previous ones on the way out, so registrations are namespaced
// per module without the modules knowing about each other.

import RefreshRuntime from "react-refresh/runtime";

RefreshRuntime.injectIntoGlobalHook(window);

// Inert defaults. A module that has not installed its own pair — anything not
// compiled with the refresh transform — must still be able to run.
window.$RefreshReg$ = () => {};
window.$RefreshSig$ = () => (type) => type;

window.__ARTIST_REFRESH__ = RefreshRuntime;

export default RefreshRuntime;
