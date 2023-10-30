use super::LoadedJarRef;
use crate::{
    BlockHashReader, BlockNumReader, HeaderProvider, ReceiptProvider, TransactionsProvider,
};
use reth_db::{
    codecs::CompactU256,
    snapshot::{HeaderMask, ReceiptMask, SnapshotCursor, TransactionMask},
};
use reth_interfaces::{
    executor::{BlockExecutionError, BlockValidationError},
    provider::ProviderError,
    RethResult,
};
use reth_primitives::{
    Address, BlockHash, BlockHashOrNumber, BlockNumber, ChainInfo, Header, Receipt, SealedHeader,
    TransactionMeta, TransactionSigned, TransactionSignedNoHash, TxHash, TxNumber, B256, U256,
};
use std::ops::{Deref, Range, RangeBounds};

/// Provider over a specific `NippyJar` and range.
#[derive(Debug)]
pub struct SnapshotJarProvider<'a> {
    /// Main snapshot segment
    jar: LoadedJarRef<'a>,
    /// Another kind of snapshot segment to help query data from the main one.
    auxiliar_jar: Option<Box<Self>>,
}

impl<'a> Deref for SnapshotJarProvider<'a> {
    type Target = LoadedJarRef<'a>;
    fn deref(&self) -> &Self::Target {
        &self.jar
    }
}

impl<'a> From<LoadedJarRef<'a>> for SnapshotJarProvider<'a> {
    fn from(value: LoadedJarRef<'a>) -> Self {
        SnapshotJarProvider { jar: value, auxiliar_jar: None }
    }
}

impl<'a> SnapshotJarProvider<'a> {
    /// Provides a cursor for more granular data access.
    pub fn cursor<'b>(&'b self) -> RethResult<SnapshotCursor<'a>>
    where
        'b: 'a,
    {
        SnapshotCursor::new(self.value(), self.mmap_handle())
    }

    /// Adds a new auxiliar snapshot to help query data from the main one
    pub fn with_auxiliar(mut self, auxiliar_jar: SnapshotJarProvider<'a>) -> Self {
        self.auxiliar_jar = Some(Box::new(auxiliar_jar));
        self
    }
}

impl<'a> HeaderProvider for SnapshotJarProvider<'a> {
    fn header(&self, block_hash: &BlockHash) -> RethResult<Option<Header>> {
        Ok(self
            .cursor()?
            .get_two::<HeaderMask<Header, BlockHash>>(block_hash.into())?
            .filter(|(_, hash)| hash == block_hash)
            .map(|(header, _)| header))
    }

    fn header_by_number(&self, num: BlockNumber) -> RethResult<Option<Header>> {
        self.cursor()?.get_one::<HeaderMask<Header>>(num.into())
    }

    fn header_td(&self, block_hash: &BlockHash) -> RethResult<Option<U256>> {
        Ok(self
            .cursor()?
            .get_two::<HeaderMask<CompactU256, BlockHash>>(block_hash.into())?
            .filter(|(_, hash)| hash == block_hash)
            .map(|(td, _)| td.into()))
    }

    fn header_td_by_number(&self, num: BlockNumber) -> RethResult<Option<U256>> {
        Ok(self.cursor()?.get_one::<HeaderMask<CompactU256>>(num.into())?.map(Into::into))
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> RethResult<Vec<Header>> {
        let range = to_range(range);

        let mut cursor = self.cursor()?;
        let mut headers = Vec::with_capacity((range.end - range.start) as usize);

        for num in range.start..range.end {
            if let Some(header) = cursor.get_one::<HeaderMask<Header>>(num.into())? {
                headers.push(header);
            }
        }

        Ok(headers)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> RethResult<Vec<SealedHeader>> {
        let range = to_range(range);

        let mut cursor = self.cursor()?;
        let mut headers = Vec::with_capacity((range.end - range.start) as usize);

        for number in range.start..range.end {
            if let Some((header, hash)) =
                cursor.get_two::<HeaderMask<Header, BlockHash>>(number.into())?
            {
                headers.push(header.seal(hash))
            }
        }
        Ok(headers)
    }

    fn sealed_header(&self, number: BlockNumber) -> RethResult<Option<SealedHeader>> {
        Ok(self
            .cursor()?
            .get_two::<HeaderMask<Header, BlockHash>>(number.into())?
            .map(|(header, hash)| header.seal(hash)))
    }
}

impl<'a> BlockHashReader for SnapshotJarProvider<'a> {
    fn block_hash(&self, number: u64) -> RethResult<Option<B256>> {
        self.cursor()?.get_one::<HeaderMask<BlockHash>>(number.into())
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> RethResult<Vec<B256>> {
        let mut cursor = self.cursor()?;
        let mut hashes = Vec::with_capacity((end - start) as usize);

        for number in start..end {
            if let Some(hash) = cursor.get_one::<HeaderMask<BlockHash>>(number.into())? {
                hashes.push(hash)
            }
        }
        Ok(hashes)
    }
}

impl<'a> BlockNumReader for SnapshotJarProvider<'a> {
    fn chain_info(&self) -> RethResult<ChainInfo> {
        // Information on live database
        Err(ProviderError::UnsupportedProvider.into())
    }

    fn best_block_number(&self) -> RethResult<BlockNumber> {
        // Information on live database
        Err(ProviderError::UnsupportedProvider.into())
    }

    fn last_block_number(&self) -> RethResult<BlockNumber> {
        // Information on live database
        Err(ProviderError::UnsupportedProvider.into())
    }

    fn block_number(&self, hash: B256) -> RethResult<Option<BlockNumber>> {
        let mut cursor = self.cursor()?;

        Ok(cursor
            .get_one::<HeaderMask<BlockHash>>((&hash).into())?
            .and_then(|res| (res == hash).then(|| cursor.number())))
    }
}

impl<'a> TransactionsProvider for SnapshotJarProvider<'a> {
    fn transaction_id(&self, hash: TxHash) -> RethResult<Option<TxNumber>> {
        let mut cursor = self.cursor()?;

        Ok(cursor
            .get_one::<TransactionMask<TransactionSignedNoHash>>((&hash).into())?
            .and_then(|res| (res.hash() == hash).then(|| cursor.number())))
    }

    fn transaction_by_id(&self, num: TxNumber) -> RethResult<Option<TransactionSigned>> {
        Ok(self
            .cursor()?
            .get_one::<TransactionMask<TransactionSignedNoHash>>(num.into())?
            .map(|tx| tx.with_hash()))
    }

    fn transaction_by_id_no_hash(
        &self,
        num: TxNumber,
    ) -> RethResult<Option<TransactionSignedNoHash>> {
        self.cursor()?.get_one::<TransactionMask<TransactionSignedNoHash>>(num.into())
    }

    fn transaction_by_hash(&self, hash: TxHash) -> RethResult<Option<TransactionSigned>> {
        Ok(self
            .cursor()?
            .get_one::<TransactionMask<TransactionSignedNoHash>>((&hash).into())?
            .map(|tx| tx.with_hash()))
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> RethResult<Option<(TransactionSigned, TransactionMeta)>> {
        // Information required on indexing table [`tables::TransactionBlock`]
        Err(ProviderError::UnsupportedProvider.into())
    }

    fn transaction_block(&self, _id: TxNumber) -> RethResult<Option<BlockNumber>> {
        // Information on indexing table [`tables::TransactionBlock`]
        Err(ProviderError::UnsupportedProvider.into())
    }

    fn transactions_by_block(
        &self,
        _block_id: BlockHashOrNumber,
    ) -> RethResult<Option<Vec<TransactionSigned>>> {
        // Related to indexing tables. Live database should get the tx_range and call snapshot
        // provider with `transactions_by_tx_range` instead.
        Err(ProviderError::UnsupportedProvider.into())
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> RethResult<Vec<Vec<TransactionSigned>>> {
        // Related to indexing tables. Live database should get the tx_range and call snapshot
        // provider with `transactions_by_tx_range` instead.
        Err(ProviderError::UnsupportedProvider.into())
    }

    fn senders_by_tx_range(&self, range: impl RangeBounds<TxNumber>) -> RethResult<Vec<Address>> {
        let txs = self.transactions_by_tx_range(range)?;
        Ok(TransactionSignedNoHash::recover_signers(&txs, txs.len())
            .ok_or(BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError))?)
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> RethResult<Vec<reth_primitives::TransactionSignedNoHash>> {
        let range = to_range(range);
        let mut cursor = self.cursor()?;
        let mut txes = Vec::with_capacity((range.end - range.start) as usize);

        for num in range {
            if let Some(tx) =
                cursor.get_one::<TransactionMask<TransactionSignedNoHash>>(num.into())?
            {
                txes.push(tx)
            }
        }
        Ok(txes)
    }

    fn transaction_sender(&self, num: TxNumber) -> RethResult<Option<Address>> {
        Ok(self
            .cursor()?
            .get_one::<TransactionMask<TransactionSignedNoHash>>(num.into())?
            .and_then(|tx| tx.recover_signer()))
    }
}

impl<'a> ReceiptProvider for SnapshotJarProvider<'a> {
    fn receipt(&self, num: TxNumber) -> RethResult<Option<Receipt>> {
        self.cursor()?.get_one::<ReceiptMask<Receipt>>(num.into())
    }

    fn receipt_by_hash(&self, hash: TxHash) -> RethResult<Option<Receipt>> {
        if let Some(tx_snapshot) = &self.auxiliar_jar {
            if let Some(num) = tx_snapshot.transaction_id(hash)? {
                return self.receipt(num)
            }
        }
        Ok(None)
    }

    fn receipts_by_block(&self, _block: BlockHashOrNumber) -> RethResult<Option<Vec<Receipt>>> {
        // Related to indexing tables. Snapshot should get the tx_range and call snapshot
        // provider with `receipt()` instead for each
        Err(ProviderError::UnsupportedProvider.into())
    }
}

fn to_range<R: RangeBounds<u64>>(bounds: R) -> Range<u64> {
    let start = match bounds.start_bound() {
        std::ops::Bound::Included(&v) => v,
        std::ops::Bound::Excluded(&v) => v + 1,
        std::ops::Bound::Unbounded => 0,
    };

    let end = match bounds.end_bound() {
        std::ops::Bound::Included(&v) => v + 1,
        std::ops::Bound::Excluded(&v) => v,
        std::ops::Bound::Unbounded => u64::MAX,
    };

    start..end
}
