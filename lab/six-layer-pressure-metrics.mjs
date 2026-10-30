/** Count durable owner-offline delivery only on intended, online, non-sender peers. */
export function ownerOfflineDelivery(nodes, attempts, recipientHistories) {
  let expectedDeliveries = 0, receivedDeliveries = 0;
  const missingDeliveries = [], perAttempt = [];
  for (const attempt of attempts) {
    const sender = nodes.find(node => node.index === attempt.sender);
    if (!sender) throw Error('unknown_pressure_sender');
    const expectedRecipients = nodes.filter(node => node.index !== 0 && node.index !== attempt.sender);
    const received = [];
    for (const recipient of expectedRecipients) {
      const rows = recipientHistories.get(recipient.index) || [];
      // Sender identity matters: a matching ID from another author is not delivery.
      const delivered = rows.some(row => row.id === attempt.id && row.sender === sender.id);
      expectedDeliveries++;
      if (delivered) { receivedDeliveries++; received.push(recipient.index); }
      else missingDeliveries.push({ id: attempt.id, sender: sender.index, recipient: recipient.index });
    }
    perAttempt.push({ id: attempt.id, sender: sender.index, expectedRecipients: expectedRecipients.map(node => node.index), receivedRecipients: received });
  }
  return {
    attemptedMessages: attempts.length, expectedDeliveries, receivedDeliveries,
    deliveryRate: expectedDeliveries ? receivedDeliveries / expectedDeliveries : 1,
    senderCopiesExcluded: true, offlineOwnerExcluded: true, missingDeliveries, perAttempt,
  };
}
