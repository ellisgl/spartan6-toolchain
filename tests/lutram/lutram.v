// Distributed RAM and shift register test (SLICEM LUTs) over the Mojo's USB
// serial port:
//   'W' addr data -> 'K'   m64[addr % 64] = m32[addr % 32] = data
//   'R' addr      -> 2 bytes: m64[addr % 64], m32[addr % 32]
//   'S' data      -> 1 byte: the byte shifted in 24 'S' commands ago
//   'P'           -> 1 byte: bit 15 of a 16-bit register initialised to
//                    16'hA5C3, which then rotates left by one (an SRL with a
//                    non-zero initial value)
// m64 and m32 have asynchronous reads (RAM64M / RAM32M); the shift register
// is 24 deep (SRLC32E). Checked by tools/lutram-test.py.
module lutram (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    output wire [7:0] led
);
    wire ready;
    mojo_ready rdy (.clk(clk), .cclk(cclk), .ready(ready));

    wire [7:0] rx_data;
    wire       rx_valid;
    uart_rx urx (.clk(clk), .rx_pin(avr_tx), .data(rx_data), .valid(rx_valid));

    reg [7:0] m64 [0:63];
    reg [7:0] m32 [0:31];
    reg [7:0] sr  [0:23];

    reg [23:0] cmd = 0;
    reg [1:0]  ncmd = 0;
    wire [7:0] op = cmd[7:0];
    reg [7:0]  addr = 0, data = 0;
    reg        we = 0, shift = 0, rot = 0;
    reg [15:0] pat = 16'hA5C3;
    always @(posedge clk)
        if (rot) pat <= {pat[14:0], pat[15]};

    always @(posedge clk) begin
        if (we) begin
            m64[addr[5:0]] <= data;
            m32[addr[4:0]] <= data;
        end
    end
    integer i;
    always @(posedge clk)
        if (shift) begin
            sr[0] <= data;
            for (i = 1; i < 24; i = i + 1)
                sr[i] <= sr[i-1];
        end
    wire [7:0] q64 = m64[addr[5:0]];
    wire [7:0] q32 = m32[addr[4:0]];
    wire [7:0] qsr = sr[23];

    reg [15:0] out = 0;
    reg [1:0]  nout = 0;
    reg [1:0]  phase = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));

    always @(posedge clk) begin
        we    <= 1'b0;
        shift <= 1'b0;
        rot   <= 1'b0;
        if (rx_valid && phase == 0) begin
            if (ncmd == 0)
                cmd <= {16'h0, rx_data};
            else
                cmd[ncmd * 8 +: 8] <= rx_data;
            ncmd <= ncmd + 1'b1;
        end
        if (phase == 0 && ncmd != 0) begin
            if (op == "W" && ncmd == 3) begin
                addr <= cmd[15:8]; data <= cmd[23:16]; we <= 1'b1;
                out <= "K"; nout <= 1; ncmd <= 0;
            end else if (op == "R" && ncmd == 2) begin
                addr <= cmd[15:8]; phase <= 1; ncmd <= 0;
            end else if (op == "S" && ncmd == 2) begin
                data <= cmd[15:8]; phase <= 2; ncmd <= 0;
            end else if (op == "P") begin
                out <= {8'h00, 7'h00, pat[15]}; nout <= 1; rot <= 1'b1; ncmd <= 0;
            end else if (op != "W" && op != "R" && op != "S") begin
                ncmd <= 0;
            end
        end
        case (phase)
            1: begin out <= {q32, q64}; nout <= 2; phase <= 0; end
            2: begin out <= {8'h00, qsr}; nout <= 1; shift <= 1'b1; phase <= 0; end
            default: ;
        endcase
        if (tx_taken) begin
            out  <= {8'h00, out[15:8]};
            nout <= nout - 1'b1;
        end
    end

    assign led = q64;
endmodule
