import test from 'node:test';
import assert from 'node:assert/strict';
import { ownerOfflineDelivery } from './six-layer-pressure-metrics.mjs';

const nodes = [0, 1, 2, 3].map(index => ({ index, id: `peer-${index}` }));
const attempts = [{ sender: 1, id: 'message-one' }, { sender: 2, id: 'message-two' }];
test('sender-local copies cannot make an owner-offline delivery test pass', () => {
  const histories = new Map([
    [1, [{ id: 'message-one', sender: 'peer-1' }]],
    [2, [{ id: 'message-two', sender: 'peer-2' }]],
    [0, [{ id: 'message-one', sender: 'peer-1' }, { id: 'message-two', sender: 'peer-2' }]],
  ]);
  const result = ownerOfflineDelivery(nodes, attempts, histories);
  assert.equal(result.expectedDeliveries, 4);
  assert.equal(result.receivedDeliveries, 0);
  assert.equal(result.deliveryRate, 0);
  assert.equal(result.missingDeliveries.length, 4);
});
test('each intended recipient counts once, author identity must match, and every target is required', () => {
  const one = { id: 'message-one', sender: 'peer-1' }, two = { id: 'message-two', sender: 'peer-2' };
  const histories = new Map([[1, [two]], [2, [one, one]], [3, [one, { ...two, sender: 'wrong-author' }]]]);
  const incomplete = ownerOfflineDelivery(nodes, attempts, histories);
  assert.equal(incomplete.receivedDeliveries, 3);
  assert.equal(incomplete.deliveryRate, 0.75);
  histories.get(3).push(two);
  const complete = ownerOfflineDelivery(nodes, attempts, histories);
  assert.equal(complete.receivedDeliveries, 4);
  assert.equal(complete.deliveryRate, 1);
  assert.equal(complete.missingDeliveries.length, 0);
});
test('a two-member group has zero online non-sender recipients when its owner is offline', () => {
  const result = ownerOfflineDelivery(nodes.slice(0, 2), [attempts[0]], new Map());
  assert.equal(result.expectedDeliveries, 0);
  assert.deepEqual(result.perAttempt[0].expectedRecipients, []);
});
