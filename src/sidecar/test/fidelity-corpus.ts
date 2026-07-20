/**
 * WP6 capture-fidelity corpus: the fixed set of (fixture, service, anchors)
 * the fidelity harness captures over. Deterministic and LLM-free — the anchors
 * are hand-authored here, not produced by the analyzer, so the whole harness
 * runs locally at zero paid spend (design plan WP6: "capture/check is
 * deterministic given anchors; unit-test locally free").
 *
 * Coverage rationale (why these three fixtures):
 *  - capture-v2-bare is the only fixture that exercises the FULL fidelity
 *    matrix: node_builder and structural_fallback tiers, the
 *    allowlisted_external and decayed_internal self-check outcomes, and the
 *    anchor-backfill origin. Its anchors mirror capture-v2-forms.test.ts
 *    (bare block), less the span-locator duplicate — expression_text locators
 *    keep the corpus spec free of fixture-source byte offsets.
 *  - capture-v2-installed adds the resolved-through-node_modules path: the
 *    same anchor shapes classify against a committed dependency instead of a
 *    bare checkout.
 *  - xrepo-corpus-3/inventory-svc is a real bare corpus service (all
 *    emitted / ok / llm-symbol), so the baseline is anchored on production
 *    fixture shapes and not only the synthetic ones.
 * Broadening to the other xrepo-corpus-3 services needs hand-authored,
 * ground-truthed anchors per service and is tracked as a follow-up, not a
 * WP6 blocker.
 *
 * The anchor definitions here are cross-referenced by capture-v2.test.ts and
 * capture-v2-forms.test.ts, which assert the same shapes at the per-alias
 * level; this module is the roll-up source, those files are the unit pins.
 */

import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import type { CaptureAnchorRequest } from '../src/capture/api.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
// dist/test -> src/sidecar/test (fixtures live under source, never compiled).
const SIDECAR_FIXTURES = path.join(__dirname, '..', '..', 'test', 'fixtures');
// dist/test -> repo root is four levels up (dist/test -> dist -> sidecar ->
// src -> root), matching capture-v2.test.ts.
const REPO_FIXTURES = path.join(
  __dirname, '..', '..', '..', '..', 'tests', 'fixtures'
);

export interface FidelityFixture {
  /** Stable key for the baseline and for cross-run ordering. */
  id: string;
  repoRoot: string;
  serviceName: string;
  /** Documented precondition: true when the fixture has no node_modules. */
  bareExpected: boolean;
  anchors: CaptureAnchorRequest[];
}

/**
 * The full-matrix synthetic bare fixture. Anchor set mirrors the bare block of
 * capture-v2-forms.test.ts (span-locator infer omitted; the text-locator infer
 * covers the same node_builder tier).
 */
const CAPTURE_V2_BARE: FidelityFixture = {
  id: 'capture-v2-bare',
  repoRoot: path.join(SIDECAR_FIXTURES, 'capture-v2-bare'),
  serviceName: 'Capture Bare Svc',
  bareExpected: true,
  anchors: [
    {
      kind: 'symbol',
      alias: 'Endpoint_order_Response',
      symbol_name: 'OrderResponse',
      source_file: 'src/http/routes.ts',
      anchor_origin: 'llm-symbol',
    },
    {
      kind: 'symbol',
      alias: 'Endpoint_order_Request',
      symbol_name: 'CreateOrderRequest',
      source_file: 'src/http/routes.ts',
      anchor_origin: 'llm-symbol',
    },
    {
      kind: 'symbol',
      alias: 'Graphql_orderquery_Result',
      symbol_name: 'OrderQueryResult',
      source_file: 'src/graphql/ops.ts',
      anchor_origin: 'llm-symbol',
    },
    {
      kind: 'handler_return',
      alias: 'Endpoint_getorder_Handler',
      symbol_name: 'getOrder',
      source_file: 'src/http/routes.ts',
      anchor_origin: 'llm-symbol',
    },
    {
      kind: 'handler_return',
      alias: 'Endpoint_overloaded_Handler',
      symbol_name: 'overloadedHandler',
      source_file: 'src/http/routes.ts',
      anchor_origin: 'llm-symbol',
    },
    {
      kind: 'handler_return',
      alias: 'Endpoint_generic_Handler',
      symbol_name: 'genericHandler',
      source_file: 'src/http/routes.ts',
      anchor_origin: 'llm-symbol',
    },
    {
      kind: 'symbol',
      alias: 'Pub_shipmentthing_Payload',
      symbol_name: 'ShipmentThing',
      source_file: 'src/events/pub.ts',
      anchor_origin: 'llm-symbol',
    },
    {
      kind: 'symbol',
      alias: 'Pub_derivedconfig_Payload',
      symbol_name: 'DerivedConfigType',
      source_file: 'src/events/pub.ts',
      anchor_origin: 'anchor-backfill',
    },
    {
      kind: 'infer',
      alias: 'Pub_ordershipped_Payload',
      source_file: 'src/events/pub.ts',
      anchor_origin: 'deterministic-infer',
      expression_text: "{ orderId: '1', eta: 'soon' }",
    },
    {
      kind: 'infer',
      alias: 'Pub_shipmentsync_Payload',
      source_file: 'src/events/pub.ts',
      anchor_origin: 'deterministic-infer',
      expression_text: 'fetchShipment()',
    },
  ],
};

/** Installed counterpart: externals resolve through committed node_modules. */
const CAPTURE_V2_INSTALLED: FidelityFixture = {
  id: 'capture-v2-installed',
  repoRoot: path.join(SIDECAR_FIXTURES, 'capture-v2-installed'),
  serviceName: 'capture-installed-svc',
  bareExpected: false,
  anchors: [
    {
      kind: 'symbol',
      alias: 'Endpoint_widget_Response',
      symbol_name: 'WidgetResponse',
      source_file: 'src/service.ts',
      anchor_origin: 'llm-symbol',
    },
    {
      kind: 'handler_return',
      alias: 'Endpoint_listwidgets_Handler',
      symbol_name: 'listWidgets',
      source_file: 'src/service.ts',
      anchor_origin: 'llm-symbol',
    },
    {
      kind: 'infer',
      alias: 'Pub_widgetupdated_Payload',
      source_file: 'src/service.ts',
      anchor_origin: 'deterministic-infer',
      expression_text: "{ widget: w, at: 'now' }",
    },
  ],
};

/** Real bare corpus service: all emitted / ok / llm-symbol. */
const INVENTORY_SVC: FidelityFixture = {
  id: 'xrepo-corpus-3-inventory-svc',
  repoRoot: path.join(REPO_FIXTURES, 'xrepo-corpus-3', 'inventory-svc'),
  serviceName: 'inventory-svc',
  bareExpected: true,
  anchors: [
    {
      kind: 'symbol',
      alias: 'Endpoint_stock_Response',
      symbol_name: 'StockLevel',
      source_file: 'src/types/stock.ts',
      anchor_origin: 'llm-symbol',
    },
    {
      kind: 'symbol',
      alias: 'Sub_stockadjust_Payload',
      symbol_name: 'StockAdjustCommand',
      source_file: 'src/types/stock.ts',
      anchor_origin: 'llm-symbol',
    },
    {
      kind: 'symbol',
      alias: 'Sub_priceupdated_Payload',
      symbol_name: 'PriceUpdatedEvent',
      source_file: 'src/types/stock.ts',
      anchor_origin: 'deterministic-infer',
    },
  ],
};

/**
 * Ordered by id (code-unit, locale-independent so the baseline is byte-stable
 * across machines); the harness and baseline preserve this order.
 */
export const FIDELITY_CORPUS: FidelityFixture[] = [
  CAPTURE_V2_BARE,
  CAPTURE_V2_INSTALLED,
  INVENTORY_SVC,
].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));

/**
 * Verdict-fidelity scaffold (WP2). Once WP2's four-bucket check classifier
 * (compatible / incompatible / unverifiable / gate-caught, pinned decision 7)
 * lands its stdio contract, author VerdictCase entries here and extend the
 * harness with a check-side pass over paired producer/consumer stubs. Left a
 * stub deliberately: WP6 does not guess WP2's wire shape (a live WP2 agent
 * owns it), and a wrong guess is worse than an empty scaffold.
 */
export interface VerdictCase {
  id: string;
  producer: FidelityFixture;
  consumer: FidelityFixture;
  expected_verdict: 'compatible' | 'incompatible' | 'unverifiable' | 'gate_caught';
}

export const VERDICT_CASES: VerdictCase[] = [];
