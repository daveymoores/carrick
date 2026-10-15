// Shape 3 on the other module system: a module-scope const assigned to the
// default export.
const store = {
  flush(key) {
    return key;
  },
};

module.exports = store;
