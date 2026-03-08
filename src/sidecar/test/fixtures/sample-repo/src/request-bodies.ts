/**
 * Request body fixture
 */

interface RequestBody {
  name: string;
  email: string;
}

interface Request {
  body: RequestBody;
}

export const createUser = async (req: Request) => {
  const payload = req.body;
  return payload;
};

export async function sendUser(user: RequestBody) {
  return fetch('/api/users', { method: 'POST', body: user });
}
