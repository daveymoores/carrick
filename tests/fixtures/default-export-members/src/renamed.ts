// Shape 3, renamed: the module exports the object under another name. Its
// members keep the name the module declares them under.
const queue = {
  drain(limit: number): number {
    return limit;
  },
};

export { queue as tasks };
