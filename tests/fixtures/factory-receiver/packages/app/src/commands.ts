import {
  createByMode,
  createClient,
  createEither,
  createReader,
  openClient,
} from "@fixture/sdk";
import { distantClient, getClient, sharedClient } from "./session";

/** Bound from a factory whose annotation names the class. */
export function showRun(id: string) {
  const client = createClient();
  return client.getRun(id);
}

/** Bound from an awaited factory whose annotation names a promise of it. */
export async function showRemoteRun(id: string) {
  const client = await openClient();
  return client.getRun(id);
}

/** Bound from a factory that returns another factory's call. */
export function listRuns() {
  const client = getClient();
  return client.listRuns();
}

/** The same, one function further out. */
export function listShared() {
  const client = sharedClient();
  return client.listRuns();
}

/** A promise of the class is not the class. */
export function showPending(id: string) {
  const client = openClient();
  return client.getRun(id);
}

/** An interface names no class. */
export function showThroughReader(id: string) {
  const reader = createReader();
  return reader.getRun(id);
}

/** A union names two classes. */
export function showEither(id: string) {
  const client = createEither(false);
  return client.getRun(id);
}

/** Two `return` statements and no annotation. */
export function showByMode(id: string) {
  const client = createByMode(false);
  return client.getRun(id);
}

/** A `let` can be bound again before the call. */
export function showRebound(id: string) {
  let client = createClient();
  return client.getRun(id);
}

/** A destructured binding is a part of the factory's value. */
export function showPart(id: string) {
  const { getRun } = createClient();
  return getRun(id);
}

/** The factory is a parameter, so nothing says which function it is. */
export function showInjected(
  id: string,
  createClient: () => { getRun(id: string): string },
) {
  const client = createClient();
  return client.getRun(id);
}

/** The chain is one function longer than is read. */
export function listDistant() {
  const client = distantClient();
  return client.listRuns();
}

/** A member of the annotated factory's result, called inline. */
export function showInline(id: string) {
  return createClient().getRun(id);
}

/** The same, awaited, from a factory that promises the class. */
export async function showInlineRemote(id: string) {
  return (await openClient()).getRun(id);
}

/** The same, through a factory that returns another factory's call. */
export function listInline() {
  return getClient().listRuns();
}

/** The factory is a parameter, so nothing says which function it is. */
export function showInlineInjected(
  createClient: () => { getRun(id: string): string },
) {
  return createClient().getRun("x");
}

/** An interface names no class. */
export function showInlineReader(id: string) {
  return createReader().getRun(id);
}

/** One level only: the member called on the member's result is not read. */
export function showInlineChained(id: string) {
  return createClient().getRun(id).toUpperCase();
}

/** The assertion states a type of its own, and it is no class. */
export function showInlineAsserted(id: string) {
  return (createClient() as ReturnType<typeof createReader>).getRun(id);
}
