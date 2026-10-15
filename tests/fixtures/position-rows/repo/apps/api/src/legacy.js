const { greet } = require('@fixture/shared');

function shout(name) {
  return greet(name).toUpperCase();
}

module.exports = { shout };
