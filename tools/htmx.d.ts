// Ambient types for the slice of htmx that assets/app.js uses, plus the app's
// own custom events. Checked only, never shipped. Keep in step with the htmx
// build vendored at assets/htmx.min.js and with the events app.js handles.

interface HtmxAjaxOptions {
  source?: Element;
  target?: Element | string | null;
  swap?: string | null;
  values?: Record<string, string>;
}

interface Htmx {
  ajax(verb: string, path: string, options?: HtmxAjaxOptions): Promise<void>;
  config: { timeout: number };
}

// app.js reaches htmx both as the bare global and through window. htmx loads
// before app.js, so both are modeled as always present.
declare const htmx: Htmx;
interface Window {
  htmx: Htmx;
  /** The accent ink derivation, implemented once in assets/prepaint.js. */
  deriveWarningInk(base: string, theme: "light" | "dark"): string;
}

// htmx:confirm fires before each request. detail.issueRequest resends it.
interface HtmxConfirmDetail {
  elt: HTMLElement;
  issueRequest(skipConfirmation: boolean): void;
}

// The request-lifecycle events app.js listens for share this detail shape.
interface HtmxRequestDetail {
  elt: HTMLElement;
  xhr?: XMLHttpRequest;
  successful?: boolean;
}

// htmx:afterSwap fires after a response is swapped in. detail.target is the
// element that received the swap. requestConfig carries the originating
// request's path and POST parameters, used by animateUndoRestore to gate
// the expand animation to /unmark responses in gaps view.
interface HtmxRequestConfig {
  path?: string;
  parameters?: Record<string, string>;
}

interface HtmxAfterSwapDetail {
  target?: Element;
  requestConfig?: HtmxRequestConfig;
}

// The app's own success event, dispatched from the HX-Trigger header on /mark.
interface MarkedDetail {
  root: string;
  rel: string;
  kind: string;
  view: string;
  name: string;
}

interface HTMLElementEventMap {
  "htmx:confirm": CustomEvent<HtmxConfirmDetail>;
  "htmx:beforeRequest": CustomEvent<HtmxRequestDetail>;
  "htmx:beforeOnLoad": CustomEvent<HtmxRequestDetail>;
  "htmx:afterRequest": CustomEvent<HtmxRequestDetail>;
  "htmx:sendError": CustomEvent<HtmxRequestDetail>;
  "htmx:timeout": CustomEvent<HtmxRequestDetail>;
  "htmx:responseError": CustomEvent<HtmxRequestDetail>;
  "htmx:afterSwap": CustomEvent<HtmxAfterSwapDetail>;
  marked: CustomEvent<MarkedDetail>;
}
