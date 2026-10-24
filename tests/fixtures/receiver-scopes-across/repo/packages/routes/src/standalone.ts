import server from 'fixture-plugin-server';

// The package also builds an app of its own for local runs, so it uses the
// server package as a value and not only through its types.
export function standalone() {
  return server();
}
