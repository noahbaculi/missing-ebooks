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
}

// htmx:confirm fires before each request; detail.issueRequest resends it.
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
  marked: CustomEvent<MarkedDetail>;
}
