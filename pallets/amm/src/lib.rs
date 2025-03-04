// Copyright 2021 Parallel Finance Developer.
// This file is part of Parallel Finance.

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! # Automatic Market Maker (AMM)
//!
//! Given any [X, Y] asset pair, "base" is the `X` asset while "quote" is the `Y` asset.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(test)]
mod mock;
mod pool_structs;

mod benchmarking;
#[cfg(test)]
mod tests;
pub mod weights;

use frame_support::pallet_prelude::*;
use frame_support::{
    dispatch::DispatchResult,
    pallet_prelude::{StorageDoubleMap, StorageValue, ValueQuery},
    traits::{Get, Hooks, IsType},
    transactional, Blake2_128Concat, PalletId, Twox64Concat,
};
use frame_system::ensure_signed;
use frame_system::pallet_prelude::OriginFor;
use orml_traits::{MultiCurrency, MultiCurrencyExtended};
pub use pallet::*;
use pool_structs::PoolLiquidityAmount;
use primitives::{Amount, Balance, CurrencyId, Rate};
use sp_runtime::traits::AccountIdConversion;
use sp_runtime::traits::IntegerSquareRoot;
use sp_runtime::ArithmeticError;
pub use sp_runtime::Perbill;
use std::convert::TryFrom;
pub use weights::WeightInfo;

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::StorageHasher;
    use frame_system::ensure_root;
    use primitives::TokenSymbol;

    #[pallet::config]
    pub trait Config<I: 'static = ()>: frame_system::Config {
        type Event: From<Event<Self, I>> + IsType<<Self as frame_system::Config>::Event>;

        /// Currency type for deposit/withdraw assets to/from amm
        /// module
        type Currency: MultiCurrencyExtended<
            Self::AccountId,
            CurrencyId = CurrencyId,
            Balance = Balance,
            Amount = Amount,
        >;

        #[pallet::constant]
        type PalletId: Get<PalletId>;

        /// Weight information for extrinsics in this pallet.
        type WeightInfo: WeightInfo;

        /// A configuration flag to enable or disable the creation of new pools by "normal" users.
        #[pallet::constant]
        type AllowPermissionlessPoolCreation: Get<bool>;

        /// Defines the fees taken out of each trade and sent back to the AMM pool,
        /// typically 0.3%.
        type LpFee: Get<Perbill>;

        /// How much the protocol is taking out of each trade.
        type ProtocolFee: Get<Perbill>;

        /// Who/where to send the protocol fees
        type ProtocolFeeReceiver: Get<Self::AccountId>;
    }

    #[pallet::error]
    pub enum Error<T, I = ()> {
        /// Pool does not exist
        PoolDoesNotExist,
        /// More liquidity than user's liquidity
        MoreLiquidity,
        /// Not a ideal price ratio
        NotAIdealPriceRatio,
        /// Pool creation has been disabled
        PoolCreationDisabled,
        /// Pool does not exist
        PoolAlreadyExists,
        /// Amount out is too small
        InsufficientAmountOut,
        /// Amount in is too small
        InsufficientAmountIn,
        /// Invalid Currency Id
        InvalidCurrencyId,
        /// Invalid LP Token
        InvalidLPToken,
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub (crate) fn deposit_event)]
    pub enum Event<T: Config<I>, I: 'static = ()> {
        /// Add liquidity into pool
        /// [sender, currency_id, currency_id]
        LiquidityAdded(T::AccountId, CurrencyId, CurrencyId),
        /// Remove liquidity from pool
        /// [sender, currency_id, currency_id]
        LiquidityRemoved(T::AccountId, CurrencyId, CurrencyId),
        /// Trade using liquidity
        /// [trader, currency_id_in, currency_id_out, rate_out_for_in]
        Trade(T::AccountId, CurrencyId, CurrencyId, Rate),
    }

    #[pallet::hooks]
    impl<T: Config<I>, I: 'static> Hooks<T::BlockNumber> for Pallet<T, I> {}

    #[pallet::pallet]
    pub struct Pallet<T, I = ()>(_);

    /// The exchange rate from the underlying to the internal collateral
    #[pallet::storage]
    pub type ExchangeRate<T, I = ()> = StorageValue<_, Rate, ValueQuery>;

    /// Accounts that deposits and withdraw assets in one or more pools
    #[pallet::storage]
    #[pallet::getter(fn liquidity_providers)]
    pub type LiquidityProviders<T: Config<I>, I: 'static = ()> = StorageNMap<
        _,
        (
            NMapKey<Blake2_128Concat, T::AccountId>,
            NMapKey<Blake2_128Concat, CurrencyId>,
            NMapKey<Blake2_128Concat, CurrencyId>,
        ),
        PoolLiquidityAmount,
        ValueQuery,
        GetDefault,
    >;

    /// A bag of liquidity composed by two different assets
    #[pallet::storage]
    #[pallet::getter(fn pools)]
    pub type Pools<T: Config<I>, I: 'static = ()> = StorageDoubleMap<
        _,
        Twox64Concat,
        CurrencyId,
        Twox64Concat,
        CurrencyId,
        PoolLiquidityAmount,
        OptionQuery,
    >;

    #[pallet::call]
    impl<T: Config<I>, I: 'static> Pallet<T, I> {
        /// Allow users to add liquidity to a given pool
        ///
        /// - `pool`: Currency pool, in which liquidity will be added
        /// - `liquidity_amounts`: Liquidity amounts to be added in pool
        /// - `minimum_amounts`: specifying its "worst case" ratio when pool already exists
        #[pallet::weight(
		T::WeightInfo::add_liquidity_non_existing_pool() // Adds liquidity in already existing account.
		.max(T::WeightInfo::add_liquidity_existing_pool()) // Adds liquidity in new account
		)]
        #[transactional]
        pub fn add_liquidity(
            origin: OriginFor<T>,
            pool: (CurrencyId, CurrencyId),
            liquidity_amounts: (Balance, Balance),
            minimum_amounts: (Balance, Balance),
        ) -> DispatchResultWithPostInfo {
            let who = ensure_signed(origin)?;
            let (is_inverted, base_asset, quote_asset) =
                Self::get_upper_currency(pool.0, pool.1).ok_or(Error::<T, I>::InvalidCurrencyId)?;

            let (base_amount, quote_amount) = match is_inverted {
                true => (liquidity_amounts.1, liquidity_amounts.0),
                false => (liquidity_amounts.0, liquidity_amounts.1),
            };
            let lp_token = CurrencyId::LPToken(
                Blake2_256::hash(&Self::account_id().encode()),
                TokenSymbol::try_from(base_asset)
                    .ok()
                    .ok_or(Error::<T, I>::InvalidCurrencyId)?,
                TokenSymbol::try_from(quote_asset)
                    .ok()
                    .ok_or(Error::<T, I>::InvalidCurrencyId)?,
            );
            Pools::<T, I>::try_mutate(
                base_asset,
                quote_asset,
                |pool_liquidity_amount| -> DispatchResultWithPostInfo {
                    if let Some(liquidity_amount) = pool_liquidity_amount {
                        let optimal_quote_amount = Self::quote(
                            base_amount,
                            liquidity_amount.base_amount,
                            liquidity_amount.quote_amount,
                        );

                        let (ideal_base_amount, ideal_quote_amount): (Balance, Balance) =
                            if optimal_quote_amount <= quote_amount {
                                (base_amount, optimal_quote_amount)
                            } else {
                                let optimal_base_amount = Self::quote(
                                    quote_amount,
                                    liquidity_amount.quote_amount,
                                    liquidity_amount.base_amount,
                                );
                                (optimal_base_amount, quote_amount)
                            };

                        let (minimum_base_amount, minimum_quote_amount) = if is_inverted {
                            (minimum_amounts.1, minimum_amounts.0)
                        } else {
                            (minimum_amounts.0, minimum_amounts.1)
                        };

                        ensure!(
                            ideal_base_amount >= minimum_base_amount
                                && ideal_quote_amount >= minimum_quote_amount
                                && ideal_base_amount <= base_amount
                                && ideal_quote_amount <= quote_amount,
                            Error::<T, I>::NotAIdealPriceRatio
                        );

                        let (base_amount, quote_amount) = (ideal_base_amount, ideal_quote_amount);
                        let total_ownership = T::Currency::total_issuance(lp_token);
                        let ownership = sp_std::cmp::min(
                            (base_amount.saturating_mul(total_ownership))
                                .checked_div(liquidity_amount.base_amount)
                                .ok_or(ArithmeticError::Overflow)?,
                            (quote_amount.saturating_mul(total_ownership))
                                .checked_div(liquidity_amount.quote_amount)
                                .ok_or(ArithmeticError::Overflow)?,
                        );

                        liquidity_amount.base_amount = liquidity_amount
                            .base_amount
                            .checked_add(base_amount)
                            .ok_or(ArithmeticError::Overflow)?;
                        liquidity_amount.quote_amount = liquidity_amount
                            .quote_amount
                            .checked_add(quote_amount)
                            .ok_or(ArithmeticError::Overflow)?;

                        liquidity_amount.lp_token = lp_token;

                        *pool_liquidity_amount = Some(liquidity_amount.clone());

                        LiquidityProviders::<T, I>::try_mutate(
                            (&who, &base_asset, &quote_asset),
                            |pool_liquidity_amount| -> DispatchResult {
                                pool_liquidity_amount.base_amount = pool_liquidity_amount
                                    .base_amount
                                    .checked_add(base_amount)
                                    .ok_or(ArithmeticError::Overflow)?;
                                pool_liquidity_amount.quote_amount = pool_liquidity_amount
                                    .quote_amount
                                    .checked_add(quote_amount)
                                    .ok_or(ArithmeticError::Overflow)?;
                                pool_liquidity_amount.lp_token = lp_token;
                                Ok(())
                            },
                        )?;

                        T::Currency::deposit(lp_token, &who, ownership)?;
                        T::Currency::transfer(base_asset, &who, &Self::account_id(), base_amount)?;
                        T::Currency::transfer(
                            quote_asset,
                            &who,
                            &Self::account_id(),
                            quote_amount,
                        )?;

                        Self::deposit_event(Event::<T, I>::LiquidityAdded(
                            who,
                            base_asset,
                            quote_asset,
                        ));
                        Ok(Some(T::WeightInfo::add_liquidity_non_existing_pool()).into())
                    } else {
                        ensure!(
                            T::AllowPermissionlessPoolCreation::get(),
                            Error::<T, I>::PoolCreationDisabled
                        );

                        let ownership = base_amount.saturating_mul(quote_amount).integer_sqrt();
                        let amm_pool = PoolLiquidityAmount {
                            base_amount,
                            quote_amount,
                            lp_token,
                        };
                        *pool_liquidity_amount = Some(amm_pool.clone());
                        LiquidityProviders::<T, I>::insert(
                            (&who, &base_asset, &quote_asset),
                            amm_pool,
                        );
                        T::Currency::deposit(lp_token, &who, ownership)?;
                        T::Currency::transfer(base_asset, &who, &Self::account_id(), base_amount)?;
                        T::Currency::transfer(
                            quote_asset,
                            &who,
                            &Self::account_id(),
                            quote_amount,
                        )?;

                        Self::deposit_event(Event::<T, I>::LiquidityAdded(
                            who,
                            base_asset,
                            quote_asset,
                        ));
                        Ok(Some(T::WeightInfo::add_liquidity_existing_pool()).into())
                    }
                },
            )
        }

        /// Allow users to remove liquidity from a given pool
        ///
        /// - `pool`: Currency pool, in which liquidity will be removed
        /// - `ownership_to_remove`: Ownership to be removed from user's ownership
        #[pallet::weight(T::WeightInfo::remove_liquidity())]
        #[transactional]
        pub fn remove_liquidity(
            origin: OriginFor<T>,
            pool: (CurrencyId, CurrencyId),
            ownership_to_remove: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let (_, base_asset, quote_asset) =
                Self::get_upper_currency(pool.0, pool.1).ok_or(Error::<T, I>::InvalidCurrencyId)?;

            Pools::<T, I>::try_mutate(
                base_asset,
                quote_asset,
                |pool_liquidity_amount| -> DispatchResult {
                    let mut liquidity_amount = pool_liquidity_amount
                        .take()
                        .ok_or(Error::<T, I>::PoolDoesNotExist)?;
                    let total_ownership = T::Currency::total_issuance(liquidity_amount.lp_token);
                    ensure!(
                        total_ownership >= ownership_to_remove,
                        Error::<T, I>::MoreLiquidity
                    );

                    let base_amount = (ownership_to_remove
                        .saturating_mul(liquidity_amount.base_amount))
                    .checked_div(total_ownership)
                    .ok_or(ArithmeticError::Underflow)?;

                    let quote_amount = (ownership_to_remove
                        .saturating_mul(liquidity_amount.quote_amount))
                    .checked_div(total_ownership)
                    .ok_or(ArithmeticError::Underflow)?;

                    liquidity_amount.base_amount = liquidity_amount
                        .base_amount
                        .checked_sub(base_amount)
                        .ok_or(ArithmeticError::Underflow)?;
                    liquidity_amount.quote_amount = liquidity_amount
                        .quote_amount
                        .checked_sub(quote_amount)
                        .ok_or(ArithmeticError::Underflow)?;

                    LiquidityProviders::<T, I>::try_mutate(
                        (&who, &base_asset, &quote_asset),
                        |pool_liquidity_amount| -> DispatchResult {
                            pool_liquidity_amount.base_amount = pool_liquidity_amount
                                .base_amount
                                .checked_sub(base_amount)
                                .ok_or(ArithmeticError::Underflow)?;
                            pool_liquidity_amount.quote_amount = pool_liquidity_amount
                                .quote_amount
                                .checked_sub(quote_amount)
                                .ok_or(ArithmeticError::Underflow)?;

                            Ok(())
                        },
                    )?;
                    T::Currency::withdraw(liquidity_amount.lp_token, &who, ownership_to_remove)?;
                    T::Currency::transfer(base_asset, &Self::account_id(), &who, base_amount)?;
                    T::Currency::transfer(quote_asset, &Self::account_id(), &who, quote_amount)?;

                    Self::deposit_event(Event::<T, I>::LiquidityRemoved(
                        who,
                        base_asset,
                        quote_asset,
                    ));

                    Ok(())
                },
            )
        }

        /// "force" the creation of a new pool by root
        ///
        /// - `pool`: Currency pool, in which liquidity will be added
        /// - `liquidity_amounts`: Liquidity amounts to be added in pool
        /// - `lptoken_receiver`: Allocate any liquidity tokens to lptoken_receiver
        #[pallet::weight(T::WeightInfo::force_create_pool())]
        #[transactional]
        pub fn force_create_pool(
            origin: OriginFor<T>,
            pool: (CurrencyId, CurrencyId),
            liquidity_amounts: (Balance, Balance),
            lptoken_receiver: T::AccountId,
        ) -> DispatchResultWithPostInfo {
            ensure_root(origin)?;

            let (is_inverted, base_asset, quote_asset) =
                Self::get_upper_currency(pool.0, pool.1).ok_or(Error::<T, I>::InvalidCurrencyId)?;

            ensure!(
                !Pools::<T, I>::contains_key(&base_asset, &quote_asset),
                Error::<T, I>::PoolAlreadyExists
            );

            let (base_amount, quote_amount) = match is_inverted {
                true => (liquidity_amounts.1, liquidity_amounts.0),
                false => (liquidity_amounts.0, liquidity_amounts.1),
            };

            let lp_token = CurrencyId::LPToken(
                Blake2_256::hash(&Self::account_id().encode()),
                TokenSymbol::try_from(base_asset)
                    .ok()
                    .ok_or(Error::<T, I>::InvalidCurrencyId)?,
                TokenSymbol::try_from(quote_asset)
                    .ok()
                    .ok_or(Error::<T, I>::InvalidCurrencyId)?,
            );

            let ownership = base_amount.saturating_mul(quote_amount).integer_sqrt();
            let amm_pool = PoolLiquidityAmount {
                base_amount,
                quote_amount,
                lp_token,
            };
            Pools::<T, I>::insert(&base_asset, &quote_asset, amm_pool.clone());
            LiquidityProviders::<T, I>::insert(
                (&lptoken_receiver, &base_asset, &quote_asset),
                amm_pool,
            );
            T::Currency::deposit(lp_token, &lptoken_receiver, ownership)?;
            T::Currency::transfer(
                base_asset,
                &lptoken_receiver,
                &Self::account_id(),
                base_amount,
            )?;
            T::Currency::transfer(
                quote_asset,
                &lptoken_receiver,
                &Self::account_id(),
                quote_amount,
            )?;

            Self::deposit_event(Event::<T, I>::LiquidityAdded(
                lptoken_receiver,
                base_asset,
                quote_asset,
            ));
            Ok(().into())
        }
    }
}

impl<T: Config<I>, I: 'static> Pallet<T, I> {
    pub fn account_id() -> T::AccountId {
        T::PalletId::get().into_account()
    }

    pub fn get_upper_currency(
        curr_a: CurrencyId,
        curr_b: CurrencyId,
    ) -> Option<(bool, CurrencyId, CurrencyId)> {
        if curr_a.is_token_currency_id() && curr_b.is_token_currency_id() {
            if curr_a > curr_b {
                Some((false, curr_a, curr_b))
            } else {
                Some((true, curr_b, curr_a))
            }
        } else {
            None
        }
    }

    pub fn quote(amount: Balance, base_pool: Balance, quote_pool: Balance) -> Balance {
        (amount.saturating_mul(quote_pool))
            .checked_div(base_pool)
            .expect("cannot overflow with positive divisor; qed")
    }
}

impl<T: Config<I>, I: 'static> primitives::AMM<T> for Pallet<T, I> {
    fn trade(
        who: &T::AccountId,
        pair: (CurrencyId, CurrencyId),
        amount_in: Balance,
        minimum_amount_out: Balance,
    ) -> Result<Balance, sp_runtime::DispatchError> {
        // expand variables
        let (input_token, output_token) = pair;

        // Sort pair to interact with the correct pool.
        let (is_inverted, base_asset, quote_asset) =
            Self::get_upper_currency(input_token, output_token)
                .ok_or(Error::<T, I>::InvalidCurrencyId)?;

        // If the pool exists, update pool base_amount and quote_amount by trade amounts
        Pools::<T, I>::try_mutate(
            &base_asset,
            &quote_asset,
            |pool_liquidity_amount| -> Result<Balance, DispatchError> {
                // 1. If the pool we want to trade does not exist in the current instance, error
                let mut liquidity_amount = pool_liquidity_amount
                    .take()
                    .ok_or(Error::<T, I>::PoolDoesNotExist)?;

                // supply_in == liquidity_amount.base_amount unless inverted
                let (supply_in, supply_out) = if is_inverted {
                    (liquidity_amount.quote_amount, liquidity_amount.base_amount)
                } else {
                    (liquidity_amount.base_amount, liquidity_amount.quote_amount)
                };

                // amount must incur at least 1 in lp fees
                ensure!(
                    amount_in >= T::LpFee::get().saturating_reciprocal_mul(1)
                        && amount_in >= T::ProtocolFee::get().saturating_reciprocal_mul(1),
                    Error::<T, I>::InsufficientAmountIn
                );

                // 2. Compute all fees to be taken out, see @Fees
                // we round down for trader convenience
                let lp_fees = T::LpFee::get().mul_floor(amount_in);
                let protocol_fees = T::ProtocolFee::get().mul_floor(amount_in);

                // subtract protocol fees from amount_in
                let amount_without_protocol_fees = amount_in
                    .checked_sub(protocol_fees)
                    .ok_or(ArithmeticError::Underflow)?;

                // subtract lp fees from amount_in minus protocol fees
                let amount_in_after_all_fees = amount_without_protocol_fees
                    .checked_sub(lp_fees)
                    .ok_or(ArithmeticError::Underflow)?;

                // 3. Given the input amount amount_in left after fees, compute amount_out
                // let amount_out = amount_in * supply_out / (supply_in + amount_in)
                let amount_out = amount_in_after_all_fees
                    .saturating_mul(supply_out)
                    .checked_div(
                        supply_in
                            .checked_add(amount_in_after_all_fees)
                            .ok_or(ArithmeticError::Overflow)?,
                    )
                    .ok_or(ArithmeticError::Underflow)?;

                // 4. If `amount_out` is lower than `min_amount_out`, error
                ensure!(
                    amount_out >= minimum_amount_out && amount_in > 0,
                    Error::<T, I>::InsufficientAmountOut
                );

                // 5. Update the `Pools` storage to track the `base_amount` and `quote_amount`
                // variables (increase and decrease by `amount_in` and `amount_out`)
                // increase liquidity_amount.base_amount by amount_in, unless inverted
                if is_inverted {
                    liquidity_amount.quote_amount = liquidity_amount
                        .quote_amount
                        .checked_add(amount_without_protocol_fees)
                        .ok_or(ArithmeticError::Overflow)?;

                    liquidity_amount.base_amount = liquidity_amount
                        .base_amount
                        .checked_sub(amount_out)
                        .ok_or(ArithmeticError::Underflow)?;
                } else {
                    liquidity_amount.base_amount = liquidity_amount
                        .base_amount
                        .checked_add(amount_without_protocol_fees)
                        .ok_or(ArithmeticError::Overflow)?;

                    liquidity_amount.quote_amount = liquidity_amount
                        .quote_amount
                        .checked_sub(amount_out)
                        .ok_or(ArithmeticError::Underflow)?;
                }
                *pool_liquidity_amount = Some(liquidity_amount);

                // 6. Wire amount_in of the input token (identified by pair.0) from who to PalletId
                T::Currency::transfer(
                    input_token,
                    who,
                    &Self::account_id(),
                    amount_without_protocol_fees,
                )?;

                // 7. Wire amount_out of the output token (identified by pair.1) to who from PalletId
                T::Currency::transfer(output_token, &Self::account_id(), who, amount_out)?;

                // 8. Wire protocol fees as needed (input token)
                T::Currency::transfer(
                    input_token,
                    who,
                    &T::ProtocolFeeReceiver::get(),
                    protocol_fees,
                )?;

                // Emit event of trade with rate calculated
                Self::deposit_event(Event::<T, I>::Trade(
                    who.clone(),
                    base_asset,
                    quote_asset,
                    amount_out
                        .checked_div(amount_in)
                        .ok_or(ArithmeticError::Underflow)?
                        .into(),
                ));

                // Return amount out for router pallet
                Ok(amount_out)
            },
        ) // return output of try_mutate as `trade` output
    }
}
