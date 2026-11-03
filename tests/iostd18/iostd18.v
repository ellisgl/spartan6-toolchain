// I/O standard LVCMOS18 (bank 0 only): outputs over its drive strengths, both
// slew rates, and inputs. Checked against ISE bitgen only (the Mojo V3's
// banks run at 3.3 V).
module iostd18 (input wire [1:0] i, output wire [7:0] o);
    assign o = {i[0], i[1], ~i[0], ~i[1], i[0] ^ i[1], i[0] & i[1], i[0] | i[1], ~(i[0] & i[1])};
endmodule
