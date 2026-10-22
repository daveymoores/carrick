import { getJson } from '@/lib/http';
import { getJson as getJsonAgain } from '../lib/http';
import { getJson as getJsonViaIndex } from '@/lib';
import type { Account } from '../types';

export async function listAccounts(): Promise<Account[]> {
  return getJson<Account[]>('/api/accounts');
}

export async function loadAccount(id: number): Promise<Account> {
  return getJsonAgain<Account>(`/api/accounts/${id}`);
}

export async function countAccounts(): Promise<number> {
  const all = await getJsonViaIndex<Account[]>('/api/accounts');
  return all.length;
}
