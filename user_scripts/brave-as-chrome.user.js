// ==UserScript==
// @name         Brave-as-Chrome (anti-fingerprint)
// @namespace    https://github.com/ryanbr/twitch-brave-fix
// @version      1.2.0
// @description  Generalized derivative of TwitchAdSolutions' twitch-brave-fix that runs on every site. Hides navigator.brave (the canonical Brave detector — exposed even in Strict shields), rebrands navigator.userAgentData.brands / getHighEntropyValues so "Brave" becomes "Google Chrome", patches navigator.onLine to track real online/offline events instead of Brave's hardwired `false` (brave/brave-browser#38240), and fabricates a plausible navigator.connection (NetworkInformation) object so sites checking `if (navigator.connection)` no longer see Brave's `undefined` (brave/brave-browser#44985). Network-level header spoofs (Sec-Ch-Ua) are NOT applied here — those would require per-site GM_xmlHttpRequest retry logic and are tightly coupled to each site's failure signature; use the original Twitch-specific script for that case. This script is JS-surface only and safe to run globally.
// @author       https://github.com/ryanbr
// @match        *://*/*
// @run-at       document-start
// @grant        none
// ==/UserScript==
(function() {
    'use strict';
    const ourVersion = 1;
    if (typeof window.braveAsChromeVersion !== 'undefined' && window.braveAsChromeVersion >= ourVersion) {
        return;
    }
    window.braveAsChromeVersion = ourVersion;

    // Snapshot is-Brave BEFORE the navigator.brave hide below, so subsequent Brave-only
    // workarounds (navigator.onLine spoof) can still gate on the original truth. Once the
    // brave property getter returns undefined, in-checks still pass (the property exists)
    // but the standard pattern (await navigator.brave.isBrave()) fails — we want both
    // outcomes simultaneously, so we capture the signal here.
    const _isBrave = ('brave' in navigator);

    // navigator.brave is the canonical Brave detector. The standard isBrave() pattern is:
    //   navigator.brave && navigator.brave.isBrave && navigator.brave.isBrave().then(...)
    // Returning undefined short-circuits the chain on the very first access. Brave exposes this
    // property even in Strict shields, so it's the most reliable detector — and the simplest to
    // neuter from a userscript. defineProperty rather than direct assignment so the getter is
    // honored even if the site uses Object.getOwnPropertyDescriptor / strict-mode access.
    try {
        if ('brave' in navigator) {
            Object.defineProperty(navigator, 'brave', {
                get: () => undefined,
                configurable: true,
            });
        }
    } catch (_e) { /* non-configurable on some Brave builds; can't help further from JS */ }

    // navigator.userAgentData.brands carries an explicit "Brave" entry on Brave. Sites that
    // call getHighEntropyValues(['brands']) bypass any outgoing-header spoof entirely and read
    // the JS-side surface directly. Rebrand both the synchronous .brands accessor and the
    // async getHighEntropyValues({brands, fullVersionList}) result. Runs synchronously at
    // document-start before site bundles can snapshot the original.
    try {
        const uaData = navigator.userAgentData;
        if (uaData) {
            const rebrand = (arr) => Array.isArray(arr)
                ? arr.map(b => b.brand === 'Brave'
                    ? { brand: 'Google Chrome', version: b.version }
                    : b)
                : arr;
            const spoofedBrands = Object.freeze(rebrand(uaData.brands));
            Object.defineProperty(uaData, 'brands', {
                get: () => spoofedBrands,
                configurable: true,
            });
            const origGHEV = uaData.getHighEntropyValues;
            if (typeof origGHEV === 'function') {
                uaData.getHighEntropyValues = function(hints) {
                    return origGHEV.call(this, hints).then(result => {
                        if (result && Array.isArray(result.brands))
                            result.brands = rebrand(result.brands);
                        if (result && Array.isArray(result.fullVersionList))
                            result.fullVersionList = rebrand(result.fullVersionList);
                        return result;
                    });
                };
            }
        }
    } catch (_e) { /* read-only on some builds — JS-surface unmaskable */ }

    // navigator.onLine is hardwired to `false` in Brave regardless of actual network state
    // (brave/brave-browser#38240). Two harms: (1) it's a Brave fingerprint (always-false
    // navigator.onLine while fetch() works = Brave), (2) it breaks legit PWA/offline-aware
    // code. Track real state via the standard online/offline events (which Brave fires
    // correctly per Chromium spec) and expose it through a replacement getter. Default
    // assumes online — page loaded, so something works — and the offline event will flip
    // the cached value the moment the OS reports a real disconnect. Gated on _isBrave so
    // Chrome/Firefox keep their native (correct) accessor untouched.
    if (_isBrave) {
        try {
            let isOnline = true;
            window.addEventListener('online', () => { isOnline = true; }, true);
            window.addEventListener('offline', () => { isOnline = false; }, true);
            Object.defineProperty(navigator, 'onLine', {
                get: () => isOnline,
                configurable: true,
            });
        } catch (_e) { /* non-configurable on some builds */ }
    }

    // navigator.connection (NetworkInformation API) is disabled entirely in Brave —
    // returns undefined regardless of shields state (brave/brave-browser#44985, gated
    // behind a feature flag). Sites detect Brave via `if (!navigator.connection)` because
    // Chrome ships this on desktop+Android. Fabricate a plausible Chrome-on-broadband
    // NetworkInformation surface so the binary detection check fails.
    //
    // Limitations of this spoof:
    //   - Values are STATIC. Brave doesn't expose real network conditions to JS, so
    //     adaptive-bitrate / data-saver code that reads these fields gets the same numbers
    //     regardless of the actual link. That's worse than nothing for accuracy, but
    //     equivalent to Brave's `undefined` for usability — code reading `effectiveType`
    //     was already broken. Detection-evasion is the gain.
    //   - The `change` event NEVER fires. addEventListener accepts the registration but
    //     no-ops since Brave doesn't surface change events to JS. Sites listening for
    //     network transitions won't see them — same observable behavior as the current
    //     `undefined` state.
    //   - Object is frozen, so `navigator.connection.onchange = handler` silently fails.
    //     Acceptable: the handler would never fire anyway, see above.
    if (_isBrave && !('connection' in navigator)) {
        try {
            const fakeConnection = Object.freeze({
                effectiveType: '4g',
                downlink: 10,
                downlinkMax: Infinity,
                rtt: 50,
                saveData: false,
                onchange: null,
                addEventListener: () => {},
                removeEventListener: () => {},
                dispatchEvent: () => false,
            });
            Object.defineProperty(navigator, 'connection', {
                get: () => fakeConnection,
                configurable: true,
            });
        } catch (_e) { /* defineProperty rejected on some builds; nothing further to do */ }
    }
})();
