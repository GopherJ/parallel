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

//! Unit tests for the prices pallet.

use super::*;
use frame_support::{assert_noop, assert_ok};
use mock::{Event, *};
use sp_runtime::{traits::BadOrigin, FixedPointNumber};

const PRICE_ONE: u128 = 1_000_000_000_000_000_000;

#[test]
fn get_price_from_oracle() {
    ExtBuilder::default().build().execute_with(|| {
        // currency exist
        assert_eq!(
            Prices::get_price(&DOT),
            Some((Price::from_inner(10_000_000_000 * PRICE_ONE), 0))
        );

        // currency not exist
        assert_eq!(
            Prices::get_price(&CurrencyId::Token(TokenSymbol::xDOT)),
            None
        );
    });
}

#[test]
fn set_price_work() {
    ExtBuilder::default().build().execute_with(|| {
        assert_eq!(
            Prices::get_price(&DOT),
            Some((Price::from_inner(10_000_000_000 * PRICE_ONE), 0))
        );
        // set DOT price
        EmergencyPrice::<Runtime>::insert(
            DOT,
            PriceWithDecimal {
                price: Price::saturating_from_integer(99),
                decimal: 10,
            },
        );
        assert_eq!(
            Prices::get_price(&DOT),
            Some((Price::from_inner(9_900_000_000 * PRICE_ONE), 0))
        );
    });
}

#[test]
fn reset_price_work() {
    ExtBuilder::default().build().execute_with(|| {
        assert_eq!(
            Prices::get_price(&DOT),
            Some((Price::from_inner(10_000_000_000 * PRICE_ONE), 0))
        );
        // set DOT price
        EmergencyPrice::<Runtime>::insert(
            DOT,
            PriceWithDecimal {
                price: Price::saturating_from_integer(99),
                decimal: 10,
            },
        );
        assert_eq!(
            Prices::get_price(&DOT),
            Some((Price::from_inner(9_900_000_000 * PRICE_ONE), 0))
        );

        // reset DOT price
        EmergencyPrice::<Runtime>::remove(DOT);
        assert_eq!(
            Prices::get_price(&DOT),
            Some((Price::from_inner(10_000_000_000 * PRICE_ONE), 0))
        );
    });
}

#[test]
fn set_price_call_work() {
    ExtBuilder::default().build().execute_with(|| {
        System::set_block_number(1);

        // set emergency price from 100 to 90
        assert_eq!(
            Prices::get_price(&DOT),
            Some((FixedU128::from_inner(10_000_000_000 * PRICE_ONE), 0))
        );
        assert_noop!(
            Prices::set_price(
                Origin::signed(2),
                DOT,
                PriceWithDecimal {
                    price: Price::saturating_from_integer(100),
                    decimal: 10
                }
            ),
            BadOrigin
        );
        assert_ok!(Prices::set_price(
            Origin::signed(1),
            DOT,
            PriceWithDecimal {
                price: Price::saturating_from_integer(90),
                decimal: 10
            }
        ));
        assert_eq!(
            Prices::get_price(&DOT),
            Some((FixedU128::from_inner(9_000_000_000 * PRICE_ONE), 0))
        );

        // check the event
        let set_price_event = Event::Prices(crate::Event::SetPrice(
            DOT,
            PriceWithDecimal {
                price: Price::saturating_from_integer(90),
                decimal: 10,
            },
        ));
        assert!(System::events()
            .iter()
            .any(|record| record.event == set_price_event));
        assert_eq!(
            Prices::set_price(
                Origin::signed(1),
                DOT,
                PriceWithDecimal {
                    price: Price::saturating_from_integer(90),
                    decimal: 10
                }
            ),
            Ok(().into())
        );
    });
}

#[test]
fn reset_price_call_work() {
    ExtBuilder::default().build().execute_with(|| {
        System::set_block_number(1);

        // set emergency price from 100 to 90
        assert_eq!(
            Prices::get_price(&DOT),
            Some((FixedU128::from_inner(10_000_000_000 * PRICE_ONE), 0))
        );
        assert_ok!(Prices::set_price(
            Origin::signed(1),
            DOT,
            PriceWithDecimal {
                price: Price::saturating_from_integer(90),
                decimal: 10
            }
        ));
        assert_eq!(
            Prices::get_price(&DOT),
            Some((FixedU128::from_inner(9_000_000_000 * PRICE_ONE), 0))
        );

        // try reset price
        assert_noop!(Prices::reset_price(Origin::signed(2), DOT), BadOrigin);
        assert_ok!(Prices::reset_price(Origin::signed(1), DOT));

        // price need to be 100 after reset_price
        assert_eq!(
            Prices::get_price(&DOT),
            Some((FixedU128::from_inner(10_000_000_000 * PRICE_ONE), 0))
        );

        // check the event
        let reset_price_event = Event::Prices(crate::Event::ResetPrice(DOT));
        assert!(System::events()
            .iter()
            .any(|record| record.event == reset_price_event));
        assert_eq!(Prices::reset_price(Origin::signed(1), DOT), Ok(().into()));
    });
}

#[test]
fn get_liquid_price_work() {
    ExtBuilder::default().build().execute_with(|| {
        assert_eq!(
            Prices::get_price(&CurrencyId::Token(TokenSymbol::KSM)),
            Some((Price::from_inner(500 * 1_000_000 * PRICE_ONE), 0))
        );

        assert_eq!(
            Prices::get_price(&CurrencyId::Token(TokenSymbol::xKSM)),
            LiquidStakingExchangeRateProvider::get_exchange_rate()
                .checked_mul_int(500 * 1_000_000 * PRICE_ONE)
                .map(|i| (Price::from_inner(i), 0))
        );
    });
}
