export async function request(url) {
  const response = await fetch(url);
  return response.json();
}
