// carrick#1841, the call inside a generated operation function. The function
// is generic in the flag that picks the result's shape, so inside it the
// call's result is a conditional type the compiler leaves deferred on the
// function's own type parameter.
import { client, type Options, type TDataShape } from './client/types';
import type {
  DeleteItemErrors,
  DeleteItemResponses,
  GetItemErrors,
  GetItemResponses,
  Item,
  ListItemsResponses,
  Problem,
} from './types.gen';

export interface Receipt {
  receipt: string;
}

export type SubmitItemResponses = {
  200: Item;
  202: Receipt;
};

export const getItemDeferred = <ThrowOnError extends boolean = false>(o: Options<TDataShape, ThrowOnError>) =>
  client.get<GetItemResponses, GetItemErrors, ThrowOnError>({ url: '/items/{id}', ...o });

export const listItemsDeferred = <ThrowOnError extends boolean = false>(o: Options<TDataShape, ThrowOnError>) =>
  client.get<ListItemsResponses, GetItemErrors, ThrowOnError>({ url: '/items', ...o });

export const deleteItemDeferred = <ThrowOnError extends boolean = false>(o: Options<TDataShape, ThrowOnError>) =>
  client.delete<DeleteItemResponses, DeleteItemErrors, ThrowOnError>({ url: '/items/{id}', ...o });

export const submitItemDeferred = <ThrowOnError extends boolean = false>(o: Options<TDataShape, ThrowOnError>) =>
  client.post<SubmitItemResponses, GetItemErrors, ThrowOnError>({ url: '/items/submit', ...o });

// The call spread over several lines, as a formatter leaves it.
export const getItemWrapped = <ThrowOnError extends boolean = false>(o: Options<TDataShape, ThrowOnError>) =>
  client.get<GetItemResponses, GetItemErrors, ThrowOnError>({
    url: '/items/{id}/wrapped',
    ...o,
  });

// A result whose two branches carry different bodies.
type Outcome<T, E> = { ok: true; value: T } | { ok: false; error: E };
declare function loadEither<Legacy extends boolean>(
  legacy: Legacy
): Legacy extends true ? Promise<Outcome<Receipt, Error>> : Promise<Outcome<Item, Error>>;

export const eitherBody = <Legacy extends boolean>(legacy: Legacy) => loadEither(legacy);

// A conditional over plain data: no branch is a result carrier.
declare function pick<T>(value: T): T extends string ? Item : Problem;

export const plainConditional = <T>(value: T) => pick(value);
