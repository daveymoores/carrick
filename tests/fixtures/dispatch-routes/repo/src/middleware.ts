// @ts-nocheck
// The minimal fixture of carrick#2139, as written there: the closure starts
// on line 10 and nothing in the repository calls it. Its two routes are
// checked on lines 12 and 20.
//
//
//
//
//
export const fake = (req, res, next) => {
  // The first route.
  if (req.method === 'GET' && req.url === '/api/one') {
    res.end('one');
    return;
  }
  //
  //
  //
  // The second route.
  if (req.method === 'POST' && req.url === '/api/two') {
    res.end('two');
    return;
  }
  next();
};
