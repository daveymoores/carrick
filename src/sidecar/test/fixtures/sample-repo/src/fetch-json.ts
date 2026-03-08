export async function loadUser() {
  const response = await fetch('https://example.com/users/1');
  const user: { id: string; name: string } = await response.json();
  return user;
}
