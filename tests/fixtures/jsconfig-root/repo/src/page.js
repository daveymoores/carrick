import { request } from '@/utils';
import { request as requestAgain } from './utils';

export async function loadPage() {
  const first = await request('/api/pages');
  const second = await requestAgain('/api/pages/home');
  return [first, second];
}
