interface Response<T> {
  json: (data: T) => void;
}

export function sendMultiline(res: Response<unknown>) {
  res.json({
    id: 'multi-1',
    name: 'Multiline',
    meta: {
      active: true,
      score: 42,
    },
  });
}
