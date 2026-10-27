const { add } = require('../src/calc');

test('adds', () => {
  expect(add(1, 2)).toBe(3);
});
